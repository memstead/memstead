//! Health checks — missing required fields, staleness, scoring.
//!
//! Checks each entity against its schema's requirements:
//! - Required metadata fields present and non-empty
//! - Required sections present and non-empty
//! - Staleness: days since last_modified > schema threshold
//! - Undeclared relationships — existing entities whose
//!   `relationships:` include a name that is not in the per-mem
//!   schema's vocabulary surface as soft warnings rather than hard
//!   load-time failures. Agents can fix either the entity or the
//!   schema; undeclared *types* on load are decision-3 hard errors
//!   and covered elsewhere.

use std::collections::HashMap;
use std::sync::Arc;

use memstead_schema::{Schema, TypeDefinition, type_by_name};

use super::{
    DanglingLink, FoldedTag, HealthIssue, HealthReport, HealthSummary, StaleEntity,
    TagDistribution, TagVariant, UntaggedStats,
};
use crate::entity::MetadataValue;
use crate::graph::query;
use crate::store::Store;

/// Allowed `include` keys for `memstead_health` — the single source of
/// truth shared across the MCP server and the
/// CLI's `health` command. Adding a new include key here lights
/// it up uniformly; agents see the same `UNKNOWN_INCLUDE_KEY` warning
/// shape whether they reach health via MCP or CLI.
pub const HEALTH_INCLUDE_KEYS: &[&str] = &[
    "orphans",
    "stubs",
    "most_connected",
    "missing_fields",
    "stale",
    "dangling_links",
    "tags",
    "missing_required_outgoing",
    "constraints",
    "signals",
    "labelling",
    "conformance",
    "integrity",
    "config",
    "anchors",
    "friction",
    "open_questions",
    "stale_derivations",
    "checks",
    "ledger",
    "vital_signs",
    "due",
];

/// Item cap per vital-signs list.
pub const VITAL_SIGNS_ITEM_CAP: usize = 20;

/// The `include=["vital_signs"]` axis (A6, 2026-09-02): per mem, the
/// cheap, engine-countable model-truth signals the remodel campaign
/// specified — each a count plus a capped list with an explicit `more`
/// remainder, never a verdict, threshold or recommendation (the engine
/// names what is there; the `/remodel` skill decides). Five signals:
///
/// - `type_share_by_community`: per community, how many of its entities
///   sit on the schema's declared last-resort type (`last_resort: true`
///   on exactly one type); `not_declared` when the schema declares none,
///   never a guess from type names.
/// - `unclaimed_source_files`: files of the mem's bound sources that no
///   entity's anchor claims, largest first with their sizes (the size
///   threshold that makes one "large" stays in the skill).
/// - `contested_unowned_files`: files two or more entities claim through
///   anchors while none owns them (no `anchored` class anchor).
/// - `zero_outgoing_entities`: entities with no outgoing edge, folded into
///   the community of their subject (their own cluster when it has more
///   members than themselves, else the cluster of an entity that links to
///   them, else `unplaced`) rather than ranked as singletons.
/// - `empty_declared_sections`: sections the type declares that an
///   entity carries empty.
///
/// Reuse, never recompute: the community partition, the anchor sidecar
/// reads and the source enumeration are the ones the other axes use.
pub fn health_vital_signs_axis(
    engine: &crate::engine::Engine,
    mem_filter: Option<&str>,
) -> serde_json::Value {
    let cap = VITAL_SIGNS_ITEM_CAP;
    let capped = |mut items: Vec<serde_json::Value>| -> serde_json::Value {
        let count = items.len();
        let more = count.saturating_sub(cap);
        items.truncate(cap);
        let mut o = serde_json::Map::new();
        o.insert("count".into(), serde_json::json!(count));
        o.insert("items".into(), serde_json::Value::Array(items));
        if more > 0 {
            o.insert("more".into(), serde_json::json!(more));
        }
        serde_json::Value::Object(o)
    };

    let mut mems: Vec<String> = engine.mem_names().iter().map(|s| s.to_string()).collect();
    mems.sort();
    let communities = engine.communities();
    let mut out = serde_json::Map::new();
    for mem in &mems {
        if let Some(f) = mem_filter
            && f != mem
        {
            continue;
        }
        let entities: Vec<&crate::entity::Entity> = engine
            .store()
            .all_entities()
            .filter(|e| !e.stub && e.id.mem() == mem)
            .collect();
        let schema = engine.schema_for(mem);

        // --- 1. type share per community ---
        let last_resort: Option<String> = schema.as_ref().and_then(|s| {
            s.types
                .values()
                .find(|t| t.last_resort)
                .map(|t| t.name.clone())
        });
        let type_share = match &last_resort {
            None => serde_json::json!({ "status": "not_declared" }),
            Some(lr) => {
                let mut per: std::collections::BTreeMap<String, (usize, usize)> =
                    std::collections::BTreeMap::new();
                for e in &entities {
                    let cluster = communities
                        .entity_cluster_map
                        .get(&e.id.0)
                        .cloned()
                        .unwrap_or_else(|| "unplaced".to_string());
                    let slot = per.entry(cluster).or_insert((0, 0));
                    slot.0 += 1;
                    if e.entity_type == *lr {
                        slot.1 += 1;
                    }
                }
                let mut rows: Vec<serde_json::Value> = per
                    .into_iter()
                    .map(|(community, (total, on_last_resort))| {
                        serde_json::json!({
                            "community": community,
                            "entities": total,
                            "on_last_resort_type": on_last_resort,
                        })
                    })
                    .collect();
                // Most concentrated first: the reader sees the flattest
                // cluster without ranking being a judgement.
                rows.sort_by(|a, b| {
                    let share = |v: &serde_json::Value| {
                        let t = v["entities"].as_u64().unwrap_or(1).max(1) as f64;
                        v["on_last_resort_type"].as_u64().unwrap_or(0) as f64 / t
                    };
                    share(b)
                        .partial_cmp(&share(a))
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a["community"].as_str().cmp(&b["community"].as_str()))
                });
                let mut v = capped(rows);
                v["status"] = serde_json::json!("declared");
                v["last_resort_type"] = serde_json::json!(lr);
                v
            }
        };

        // --- 2 and 3. the file-to-entity map of the bound sources ---
        // artifact -> (claiming entities, owned by an `anchored` anchor)
        let mut claims: std::collections::BTreeMap<
            String,
            (std::collections::BTreeSet<String>, bool),
        > = std::collections::BTreeMap::new();
        for e in &entities {
            for a in engine.entity_anchors(&e.id) {
                let slot = claims.entry(a.artifact.clone()).or_default();
                slot.0.insert(e.id.0.clone());
                if a.class == crate::anchor::AnchorProvenanceClass::Anchored {
                    slot.1 = true;
                }
            }
        }
        let roots = engine.anchor_source_roots(mem);
        let mut unclaimed: Vec<serde_json::Value> = Vec::new();
        let mut sources_enumerated = 0usize;
        if let Some(ws) = engine.workspace_root() {
            let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for join in roots.values() {
                sources_enumerated += 1;
                for file in crate::source_scope::enumerate_source_artifacts(
                    engine,
                    &join.source,
                    &join.deny_paths,
                    ws,
                ) {
                    if !seen.insert(file.clone()) || claims.contains_key(&file) {
                        continue;
                    }
                    let size = std::fs::metadata(ws.join(&file))
                        .map(|m| m.len())
                        .unwrap_or(0);
                    unclaimed.push(serde_json::json!({ "artifact": file, "bytes": size }));
                }
            }
        }
        unclaimed.sort_by(|a, b| {
            b["bytes"]
                .as_u64()
                .cmp(&a["bytes"].as_u64())
                .then_with(|| a["artifact"].as_str().cmp(&b["artifact"].as_str()))
        });
        let unclaimed_v = if sources_enumerated == 0 {
            serde_json::json!({ "status": "no_bound_source" })
        } else {
            let mut v = capped(unclaimed);
            v["status"] = serde_json::json!("enumerated");
            v
        };
        let contested: Vec<serde_json::Value> = claims
            .iter()
            .filter(|(_, (who, owned))| who.len() >= 2 && !owned)
            .map(|(artifact, (who, _))| {
                serde_json::json!({
                    "artifact": artifact,
                    "claimed_by": who.iter().cloned().collect::<Vec<_>>(),
                })
            })
            .collect();

        // --- 4. zero-outgoing entities, folded into their subject ---
        let cluster_size = |c: &str| -> usize {
            communities
                .clusters
                .get(c)
                .map(|ci| ci.entities.len())
                .unwrap_or(0)
        };
        let mut by_community: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for e in &entities {
            if !engine.store().outgoing(&e.id).is_empty() {
                continue;
            }
            let own = communities.entity_cluster_map.get(&e.id.0).cloned();
            let community = match own {
                Some(c) if cluster_size(&c) > 1 => c,
                _ => engine
                    .store()
                    .incoming(&e.id)
                    .iter()
                    .find_map(|edge| communities.entity_cluster_map.get(&edge.from.0).cloned())
                    .unwrap_or_else(|| "unplaced".to_string()),
            };
            by_community
                .entry(community)
                .or_default()
                .push(e.id.0.clone());
        }
        let zero_total: usize = by_community.values().map(Vec::len).sum();
        let zero_rows: Vec<serde_json::Value> = by_community
            .into_iter()
            .map(|(community, mut ids)| {
                ids.sort();
                let count = ids.len();
                let more = count.saturating_sub(cap);
                ids.truncate(cap);
                let mut o = serde_json::json!({
                    "community": community,
                    "count": count,
                    "items": ids,
                });
                if more > 0 {
                    o["more"] = serde_json::json!(more);
                }
                o
            })
            .collect();
        let mut zero_v = capped(zero_rows);
        zero_v["entities"] = serde_json::json!(zero_total);

        // --- 5. declared sections carried empty ---
        let mut empty_sections: Vec<serde_json::Value> = Vec::new();
        if let Some(s) = &schema {
            for e in &entities {
                let Some(td) = s.types.get(&e.entity_type) else {
                    continue;
                };
                for sec in &td.sections {
                    if e.sections
                        .get(&sec.key)
                        .is_some_and(|body| body.trim().is_empty())
                    {
                        empty_sections.push(serde_json::json!({
                            "id": e.id.0,
                            "section": sec.key,
                        }));
                    }
                }
            }
        }

        out.insert(
            mem.clone(),
            serde_json::json!({
                "type_share_by_community": type_share,
                "unclaimed_source_files": unclaimed_v,
                "contested_unowned_files": capped(contested),
                "zero_outgoing_entities": zero_v,
                "empty_declared_sections": capped(empty_sections),
            }),
        );
    }
    let mut top = serde_json::Map::new();
    top.insert("_item_cap".into(), serde_json::json!(cap));
    for (k, v) in out {
        top.insert(k, v);
    }
    serde_json::Value::Object(top)
}

/// The `include=["anchors"]` axis — per-mem counts of the four
/// standalone-verification states, computed through the same
/// per-anchor mechanism `verify-anchors` and the binding verify use.
/// Shared by the health composer, the CLI health command, and the
/// MCP server so the axis cannot drift between surfaces.
/// The `include=["checks"]` axis: per mem,
/// counts of the four derived check states plus the author≠checker
/// independence gate over ok-checked entities. The gate compares
/// caller-declared IDENTITIES and nothing else:
/// the entity's created-by record against its newest ok-check record
/// — both carry an identity and they are equal → `self_checked`
/// ("twice-asserted, not verified"); both carry one and they differ →
/// `confirmed_independent`; either side lacks one → `unconfirmable`,
/// never a guessed category. Transport is not identity: the recorded
/// `(actor, client)` pair names the SURFACE a record arrived through
/// and is recorded as context only — it never participates in the
/// comparison (a same pair does not establish the same actor, and
/// different pairs do not establish different actors). An identity is
/// an honesty device, not authentication: caller-declared, unverified,
/// tamper-evident in append-only history — a consistent multi-agent
/// setup gets real independence signals, and a lying setup defeats
/// only itself. Derivation only: nothing here is stamped, and a
/// workspace without a check ledger serves all-never-checked.
/// Identity lists are capped at [`OPEN_QUESTIONS_ITEM_CAP`] with an
/// explicit `more` count.
pub fn health_checks_axis(
    engine: &crate::engine::Engine,
    mem_filter: Option<&str>,
) -> serde_json::Value {
    let cap = OPEN_QUESTIONS_ITEM_CAP;
    let capped = |mut items: Vec<String>| -> serde_json::Value {
        items.sort();
        let count = items.len();
        let more = count.saturating_sub(cap);
        items.truncate(cap);
        let mut o = serde_json::Map::new();
        o.insert("count".into(), serde_json::json!(count));
        o.insert("items".into(), serde_json::json!(items));
        if more > 0 {
            o.insert("more".into(), serde_json::json!(more));
        }
        serde_json::Value::Object(o)
    };

    let ledger = engine
        .workspace_root()
        .map(crate::check::CheckLedger::for_workspace);
    // Newest record per (entity, kind), one ledger read for the whole
    // axis. State is kind-scoped: a conformance record never
    // supersedes a verification record, or the reverse.
    let mut latest: std::collections::BTreeMap<String, crate::check::CheckRecord> =
        std::collections::BTreeMap::new();
    let mut latest_conformance: std::collections::BTreeMap<String, crate::check::CheckRecord> =
        std::collections::BTreeMap::new();
    // Foreign `x-<name>` kinds: recorded verbatim, never aggregated into
    // a state — listed by count per mem so a reader sees that another
    // checker has been here. Keyed by entity for the per-mem tally.
    let mut foreign_by_entity: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    // The newest record of ANY kind per entity, for the finding it may
    // carry: a finding is served under the entity's latest verdict.
    let mut newest_any: std::collections::BTreeMap<String, crate::check::CheckRecord> =
        std::collections::BTreeMap::new();
    // Every verification record per entity, oldest first: the per-record
    // readings (engine::independence) show each check's standing, not
    // only the newest one's — a superseded self-check stays visible.
    let mut all_verification: std::collections::BTreeMap<String, Vec<crate::check::CheckRecord>> =
        std::collections::BTreeMap::new();
    if let Some(l) = &ledger {
        for rec in l.all() {
            newest_any.insert(rec.entity.clone(), rec.clone());
            match rec.resolved_kind() {
                Some(crate::check::CheckKind::Verification) => {
                    all_verification
                        .entry(rec.entity.clone())
                        .or_default()
                        .push(rec.clone());
                    latest.insert(rec.entity.clone(), rec);
                }
                Some(crate::check::CheckKind::Conformance) => {
                    latest_conformance.insert(rec.entity.clone(), rec);
                }
                None => {
                    if let Some(k) = rec.foreign_kind() {
                        foreign_by_entity
                            .entry(rec.entity.clone())
                            .or_default()
                            .push(k.to_string());
                    }
                }
            }
        }
    }

    let mut mems: Vec<String> = engine.mem_names().iter().map(|s| s.to_string()).collect();
    mems.sort();
    let mut out = serde_json::Map::new();
    for mem in mems {
        if let Some(f) = mem_filter
            && f != mem
        {
            continue;
        }
        let mut counts = std::collections::BTreeMap::from([
            ("never_checked", 0usize),
            ("checked_ok", 0usize),
            ("check_failed", 0usize),
            ("check_stale", 0usize),
        ]);
        // The `conformance` kind's counts, additively beside the
        // verification counts. Pin-aware: a schema re-pin stales
        // every conformance verdict recorded under the old pin. A
        // workspace with no conformance records serves all
        // `never_checked` — honestly empty, never absent.
        let current_pin = engine
            .mount(&mem)
            .and_then(|m| m.schema.as_ref())
            .map(|s| s.as_display());
        let mut conformance_counts = std::collections::BTreeMap::from([
            ("never_checked", 0usize),
            ("checked_ok", 0usize),
            ("check_failed", 0usize),
            ("check_stale", 0usize),
        ]);
        let mut self_checked: Vec<String> = Vec::new();
        let mut confirmed_independent: Vec<String> = Vec::new();
        let mut unconfirmable: Vec<String> = Vec::new();
        // Which identities each ok-checked criterion was compared against
        // (engine::independence), rendered so a reader can see who counted
        // as an executor. One provenance pass per mem.
        let mut executors = serde_json::Map::new();
        let mut readings = serde_json::Map::new();
        let touches = engine.mem_touches(&mem);
        let mut foreign_kinds: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        let mut findings = serde_json::Map::new();
        for e in engine.store().all_entities().filter(|e| e.mem == mem) {
            let id = e.id.0.clone();
            if let Some(kinds) = foreign_by_entity.get(&id) {
                for k in kinds {
                    *foreign_kinds.entry(k.clone()).or_insert(0) += 1;
                }
            }
            if let Some(rec) = newest_any.get(&id)
                && let Some(f) = &rec.finding
            {
                findings.insert(
                    id.clone(),
                    serde_json::json!({
                        "verdict": rec.verdict,
                        "kind": rec.kind.as_deref().unwrap_or("verification"),
                        "ts": rec.ts,
                        "identity": rec.identity,
                        "finding": f,
                    }),
                );
            }
            let state = crate::check::derive_state(latest.get(&id), &e.content_hash);
            *counts.entry(state.as_str()).or_insert(0) += 1;
            if let Some(records) = all_verification.get(&id) {
                let rows: Vec<serde_json::Value> = records
                    .iter()
                    .map(|rec| {
                        let reading = if rec.verdict == "ok" {
                            engine.independence_of(e, rec, &touches).0.as_str()
                        } else {
                            "failed"
                        };
                        serde_json::json!({
                            "ts": rec.ts,
                            "identity": rec.identity,
                            "verdict": rec.verdict,
                            "reading": reading,
                        })
                    })
                    .collect();
                readings.insert(id.clone(), serde_json::Value::Array(rows));
            }
            let cstate = crate::check::derive_state_pinned(
                latest_conformance.get(&id),
                &e.content_hash,
                current_pin.as_deref(),
            );
            *conformance_counts.entry(cstate.as_str()).or_insert(0) += 1;
            if state != crate::check::CheckState::CheckedOk {
                continue;
            }
            // Identity-only comparison (plan 15, comparator widened
            // 2026-09-02): the newest ok-check's identity against every
            // identity that mutated the verified plan, its criteria or
            // its session-log notes since this criterion was written
            // (engine::independence); a non-criterion compares against
            // its own author. The (actor, client) transport pair is
            // recorded context and never a comparator. Either side
            // lacking an identity is unconfirmable, never a guessed
            // category.
            let check = latest.get(&id).expect("checked_ok implies a record");
            let (reading, execs) = engine.independence_of(e, check, &touches);
            if let Some(execs) = execs {
                executors.insert(id.clone(), serde_json::json!(execs.identities));
            }
            match reading {
                crate::engine::independence::Independence::SelfChecked => self_checked.push(id),
                crate::engine::independence::Independence::ConfirmedIndependent => {
                    confirmed_independent.push(id)
                }
                crate::engine::independence::Independence::Unconfirmable => unconfirmable.push(id),
            }
        }
        let mut m = serde_json::Map::new();
        for (k, v) in counts {
            m.insert(k.to_string(), serde_json::json!(v));
        }
        let mut c = serde_json::Map::new();
        for (k, v) in conformance_counts {
            c.insert(k.to_string(), serde_json::json!(v));
        }
        m.insert("conformance".into(), serde_json::Value::Object(c));
        // Foreign kinds by count, and the structured finding each
        // entity's newest record carries — recorded, rendered, never
        // interpreted.
        m.insert(
            "foreign_kinds".into(),
            serde_json::to_value(&foreign_kinds).unwrap_or(serde_json::json!({})),
        );
        m.insert("findings".into(), serde_json::Value::Object(findings));
        m.insert(
            "independence".into(),
            serde_json::json!({
                "self_checked": capped(self_checked),
                "confirmed_independent": capped(confirmed_independent),
                "unconfirmable": capped(unconfirmable),
                "comparator": "every identity that mutated the verified plan, its criteria or its session-log notes since the criterion was written; a non-criterion compares against its own author",
                "executors": serde_json::Value::Object(executors),
                "readings": serde_json::Value::Object(readings),
            }),
        );
        out.insert(mem, serde_json::Value::Object(m));
    }
    serde_json::Value::Object(out)
}

/// One derivation-staleness finding: an
/// explicit edge on a derivation-declared rel-type whose baseline
/// differs from the target's current hash (`stale`), or that has no
/// recorded baseline at all (`unbaselined`). Fresh edges are never
/// reported.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DerivationFinding {
    pub source: crate::entity::EntityId,
    pub rel_type: String,
    pub target: crate::entity::EntityId,
    /// `"stale"` or `"unbaselined"` — never fabricated as fresh.
    pub state: String,
    /// The recorded baseline hash (`None` for unbaselined edges).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
    /// The target's current content hash ("" for an absent target).
    pub current: String,
}

/// The `include=["stale_derivations"]` axis: per-mem findings from
/// [`crate::engine::Engine::derivation_report`], shared by the CLI
/// and the MCP server. A mem whose schema declares no derivation
/// rel-types contributes an empty list — never an error.
pub fn health_stale_derivations_axis(
    engine: &crate::engine::Engine,
    mem_filter: Option<&str>,
) -> serde_json::Value {
    let mut mems: Vec<String> = engine.mem_names().iter().map(|s| s.to_string()).collect();
    mems.sort();
    let mut out = serde_json::Map::new();
    for mem in mems {
        if let Some(f) = mem_filter
            && f != mem
        {
            continue;
        }
        let findings = engine.derivation_report(&mem).unwrap_or_default();
        out.insert(
            mem,
            serde_json::to_value(&findings).unwrap_or(serde_json::Value::Array(Vec::new())),
        );
    }
    serde_json::Value::Object(out)
}

/// Per-kind item cap for the `open_questions` axis — the axis is an
/// agent worklist, not a dump. Stated in the output (`_item_cap`);
/// truncation is always explicit via each list's `more` count.
pub const OPEN_QUESTIONS_ITEM_CAP: usize = 20;

/// The `include=["open_questions"]` axis: per
/// mem, a composed worklist of what the holding does not know — its
/// stubs, its never-confirmed (`recheck`) and `unresolvable` anchors,
/// its unsatisfied constraints, its dangling links, and, when a
/// paired process mem is resolvable for the destination, that
/// process mem's open entries. Negative findings ride under the
/// DISTINCT `already_searched` heading — their operational meaning is
/// "done, keep off", never todo.
///
/// Composition only: every signal is read from the same source its
/// own axis serves (store stub flags, `verify_mem_anchors`,
/// `constraint_findings`, `collect_dangling_links`, the pipeline
/// store), so this axis can never disagree with the per-signal axes.
/// Best-effort on the process leg: an unreadable pipeline store means
/// no process sections, never an axis failure.
pub fn health_open_questions_axis(
    engine: &crate::engine::Engine,
    mem_filter: Option<&str>,
) -> serde_json::Value {
    let cap = OPEN_QUESTIONS_ITEM_CAP;
    let capped = |mut items: Vec<serde_json::Value>| -> serde_json::Value {
        let count = items.len();
        let more = count.saturating_sub(cap);
        items.truncate(cap);
        let mut o = serde_json::Map::new();
        o.insert("count".into(), serde_json::json!(count));
        o.insert("items".into(), serde_json::Value::Array(items));
        if more > 0 {
            o.insert("more".into(), serde_json::json!(more));
        }
        serde_json::Value::Object(o)
    };

    // Bindings by destination mem — the pairing plan 14 will make
    // declarative; until then the ingest-name convention (process mem
    // named after the binding) is the resolution mechanism.
    let bindings: Vec<(String, String)> = engine
        .workspace_root()
        .and_then(|root| crate::pipeline_store::load_pipeline_configs(root).ok())
        .map(|c| {
            c.bindings
                .iter()
                .map(|r| (r.config.destination_mem.clone(), r.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let mounted: Vec<String> = engine.mem_names().iter().map(|s| s.to_string()).collect();

    let mut mems: Vec<String> = mounted.clone();
    mems.sort();
    let mut out = serde_json::Map::new();
    for mem in &mems {
        if let Some(f) = mem_filter
            && f != mem
        {
            continue;
        }

        // Stubs — same source as the stubs axis (store stub flag).
        let stubs = capped(
            engine
                .store()
                .all_entities()
                .filter(|e| e.stub && e.id.mem() == mem)
                .map(|e| serde_json::json!({ "kind": "stub", "id": e.id.to_string() }))
                .collect(),
        );

        // Anchors — same per-anchor mechanism as the anchors axis;
        // only the never-confirmed and unreachable states are holes.
        // A dangling sidecar row is a hole too (consistency-sweep 03/02), and
        // it gets its OWN bucket. `unresolvable` is the wire form of the
        // orphaned state, whose repair is the opposite one: an orphaned anchor
        // asks whether its entity should be re-anchored or pruned, a dangling
        // row asks why the entity went missing. Folding the two together here
        // would reproduce, on the health axis, exactly the collapse the axis
        // itself refuses.
        //
        // `entity_end_unreconciled` rides along both ways: an empty dangling
        // bucket means "none found" only when the check could run at all.
        let (mut recheck, mut unresolvable, mut unobserved, mut dangling_rows) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        // Rows resting on a recorded observation older than today: open
        // work too (someone has to look again), stated as its age.
        let mut aging = Vec::new();
        let mut entity_end_unreconciled: Option<String> = None;
        if let Ok(report) = engine.verify_mem_anchors(mem) {
            entity_end_unreconciled = report.unreconciled.clone();
            for a in &report.anchors {
                if let Some(days) = a.unobserved_for_days
                    && days > 0
                {
                    aging.push(serde_json::json!({
                        "kind": "anchor_aging",
                        "id": a.entity_id,
                        "artifact": a.artifact,
                        "state": a.state,
                        "observed_at": a.observed_at,
                        "unobserved_for_days": days,
                        "note": format!("unobserved for {days} days"),
                    }));
                }
                let item = serde_json::json!({
                    "kind": format!("anchor_{}", a.state),
                    "id": a.entity_id,
                    "artifact": a.artifact,
                });
                match a.state.as_str() {
                    "recheck" => recheck.push(item),
                    // The enum's wire name for a gone artifact; the bucket
                    // keeps the axis's count name (`anchors_unresolvable`).
                    "orphaned" => unresolvable.push(item),
                    "unobserved" => unobserved.push(item),
                    "dangling" => dangling_rows.push(item),
                    _ => {}
                }
            }
        }

        // Unsatisfied constraints — same collector as the
        // constraints axis.
        let constraints = capped(
            engine
                .constraint_findings(Some(mem))
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "kind": "unsatisfied_constraint",
                        "id": r.id.to_string(),
                        "violations": r.violations.len(),
                    })
                })
                .collect(),
        );

        // Dangling links — same collector as the overview include, and the
        // SAME three names rather than a parallel vocabulary of its own.
        // This axis emitted one `dangling_link` kind
        // over all three conditions, which is the fused code by another
        // spelling; a second vocabulary is the one that drifts first.
        let dangling = capped(
            collect_dangling_links(engine.store(), Some(mem))
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "kind": d.kind.code(),
                        "id": d.from.to_string(),
                        "target": d.target_id.to_string(),
                        "repair": d.kind.repair(),
                    })
                })
                .collect(),
        );

        // Paired process mems: open entries are work; negative
        // findings are the opposite — already searched, keep off.
        // Pairing runs through the ONE resolution function the brief
        // renderer uses: a destination's
        // declaration wins regardless of naming — and pairs even
        // with no binding at all (the process tier stands without
        // one); the binding-name convention remains the fallback. A
        // declaration naming an unmounted mem is a typed finding,
        // never a silent fallback.
        let mut process = Vec::new();
        let mem_bindings: Vec<&String> = bindings
            .iter()
            .filter(|(d, _)| d == mem)
            .map(|(_, b)| b)
            .collect();
        let mut resolutions: Vec<(Option<String>, crate::binding_run::ProcessMemResolution)> =
            Vec::new();
        if mem_bindings.is_empty() {
            let r = crate::binding_run::resolve_process_mem(engine, mem, "");
            if r.declared {
                resolutions.push((None, r));
            }
        } else {
            for binding in &mem_bindings {
                resolutions.push((
                    Some((*binding).clone()),
                    crate::binding_run::resolve_process_mem(engine, mem, binding),
                ));
            }
        }
        for (binding, r) in resolutions {
            if r.mounted {
                let mut open = Vec::new();
                let mut searched = Vec::new();
                for e in engine
                    .store()
                    .all_entities()
                    .filter(|e| !e.stub && e.id.mem() == r.mem.as_str())
                {
                    let item = serde_json::json!({
                        "kind": e.entity_type,
                        "id": e.id.to_string(),
                        "title": e.title,
                    });
                    if e.entity_type == "negative_finding" {
                        searched.push(item);
                    } else {
                        open.push(item);
                    }
                }
                // The store iterates in hash order; a byte-parity gate over
                // this axis (the CLI-versus-MCP pin, a fixture diff between
                // two builds) needs the same bytes every run, so both lists
                // are ordered by id before the cap takes the head.
                let by_id = |a: &serde_json::Value, b: &serde_json::Value| {
                    a["id"].as_str().cmp(&b["id"].as_str())
                };
                open.sort_by(by_id);
                searched.sort_by(by_id);
                process.push(serde_json::json!({
                    "binding": binding,
                    "process_mem": r.mem,
                    "declared": r.declared,
                    "resolvable": true,
                    "open_entries": capped(open),
                    "already_searched": capped(searched),
                }));
            } else if r.declared {
                process.push(serde_json::json!({
                    "binding": binding,
                    "process_mem": r.mem,
                    "declared": true,
                    "resolvable": false,
                    "finding": "DECLARED_PROCESS_MEM_MISSING",
                }));
            } else {
                process.push(serde_json::json!({
                    "binding": binding,
                    "resolvable": false,
                }));
            }
        }

        // The resolution readings (plan B5): for every type of this mem
        // that declares `resolution`, the open entities with no condition
        // written, and the open entities whose condition nobody has
        // checked under the declared kind. The ledger is read as it is;
        // an `x-` kind counts by name, an engine kind by derived state.
        let (mut missing, mut unchecked) = (Vec::new(), Vec::new());
        if let Some(schema) = engine.schema_for(mem) {
            let ledger = engine
                .workspace_root()
                .map(crate::check::CheckLedger::for_workspace);
            for entity in engine
                .store()
                .all_entities()
                .filter(|e| !e.stub && e.id.mem() == mem)
            {
                let Some(td) = schema.types.get(&entity.entity_type) else {
                    continue;
                };
                let Some(res) = &td.resolution else { continue };
                if let Some(field) = &res.status_field {
                    let open = match entity.metadata.get(field) {
                        Some(crate::entity::MetadataValue::String(s)) => {
                            res.open_values.contains(s)
                        }
                        _ => false,
                    };
                    if !open {
                        continue;
                    }
                }
                let has_condition = entity
                    .sections
                    .get(&res.condition_section)
                    .is_some_and(|body| !body.trim().is_empty());
                if !has_condition {
                    missing.push(serde_json::json!({
                        "kind": "resolution_missing",
                        "id": entity.id.to_string(),
                        "section": res.condition_section,
                    }));
                    continue;
                }
                let kind = res.check_kind.as_deref().unwrap_or("verification");
                let checked = ledger.as_ref().is_some_and(|l| {
                    l.all().into_iter().rev().any(|r| {
                        r.entity == entity.id.to_string()
                            && r.verdict == "ok"
                            && r.entity_hash == entity.content_hash
                            && match crate::check::RecordKind::from_wire(kind) {
                                Some(crate::check::RecordKind::Engine(k)) => {
                                    r.resolved_kind() == Some(k)
                                }
                                Some(crate::check::RecordKind::Foreign(name)) => {
                                    r.kind.as_deref() == Some(name.as_str())
                                }
                                None => false,
                            }
                    })
                });
                if !checked {
                    unchecked.push(serde_json::json!({
                        "kind": "resolution_unchecked",
                        "id": entity.id.to_string(),
                        "section": res.condition_section,
                        "check_kind": kind,
                    }));
                }
            }
        }
        let resolution_missing = capped(missing);
        let resolution_unchecked = capped(unchecked);

        let total_open = stubs["count"].as_u64().unwrap_or(0)
            + resolution_missing["count"].as_u64().unwrap_or(0)
            + resolution_unchecked["count"].as_u64().unwrap_or(0)
            + recheck.len() as u64
            + unresolvable.len() as u64
            + unobserved.len() as u64
            + dangling_rows.len() as u64
            + aging.len() as u64
            + constraints["count"].as_u64().unwrap_or(0)
            + dangling["count"].as_u64().unwrap_or(0)
            + process
                .iter()
                .filter_map(|p| p["open_entries"]["count"].as_u64())
                .sum::<u64>();

        let mut entry = serde_json::Map::new();
        entry.insert("stubs".into(), stubs);
        entry.insert("anchors_recheck".into(), capped(recheck));
        entry.insert("anchors_unresolvable".into(), capped(unresolvable));
        // Its own bucket here too. The comment above argues that folding two
        // anchor conditions together reproduces the collapse the axis refuses,
        // and a first version then did exactly that eight lines further down.
        entry.insert("anchors_unobserved".into(), capped(unobserved));
        entry.insert("anchors_dangling".into(), capped(dangling_rows));
        entry.insert("anchors_aging".into(), capped(aging));
        if let Some(why) = entity_end_unreconciled {
            entry.insert("entity_end_unreconciled".into(), serde_json::json!(why));
        }
        entry.insert("unsatisfied_constraints".into(), constraints);
        entry.insert("dangling_links".into(), dangling);
        entry.insert("resolution_missing".into(), resolution_missing);
        entry.insert("resolution_unchecked".into(), resolution_unchecked);
        if !process.is_empty() {
            entry.insert("process".into(), serde_json::Value::Array(process));
        } else {
            // No binding targets this mem: the absence of a process
            // section is stated, never silent.
            entry.insert("process_mem_resolvable".into(), serde_json::json!(false));
        }
        entry.insert("total_open".into(), serde_json::json!(total_open));
        out.insert(mem.clone(), serde_json::Value::Object(entry));
    }
    let mut top = serde_json::Map::new();
    top.insert("_item_cap".into(), serde_json::json!(cap));
    for (k, v) in out {
        top.insert(k, v);
    }
    serde_json::Value::Object(top)
}

pub fn health_anchors_axis(
    engine: &crate::engine::Engine,
    mem_filter: Option<&str>,
) -> serde_json::Value {
    let mut mems: Vec<String> = engine.mem_names().iter().map(|s| s.to_string()).collect();
    mems.retain(|m| mem_filter.is_none_or(|v| m == v));
    mems.sort();
    let mut out = serde_json::Map::new();
    for mem in mems {
        let Ok(report) = engine.verify_mem_anchors(&mem) else {
            continue;
        };
        let condition = report.sidecar_error.as_ref().map(|why| {
            serde_json::json!({
                "code": "ANCHORS_SIDECAR_UNREADABLE",
                "mem": mem,
                "reason": why,
            })
        });
        // The figure travels as its type's own three fields (`resolves`,
        // `population`, `fully_adjudicated`), merged into the row.
        let mut row = serde_json::json!({
                // The one condition that replaces the counts: an unreadable
                // sidecar. Absent when the sidecar read cleanly.
                "condition": condition,
                "drifted": report.drifted,
                "recheck": report.recheck,
                // Split from `unresolvable`: the artifact
                // being gone is a measurement, the pass not reaching it is the
                // absence of one, and this is the surface a reader arrives at
                // without a binding in hand.
                "unresolvable": report.unresolvable,
                "unobserved": report.unobserved,
                // The entity end (03/02). Carried here too, both ways: four
                // counts over a mem whose sidecar has outlived its entities
                // read as a healthy axis, and so does a zero over a mem whose
                // entity end was never reconciled.
                "dangling": report.dangling,
                "entity_end_unreconciled": report.unreconciled,
                // Rows whose state rests on a recorded observation (url
                // rows), each with how long it has gone unobserved. A state
                // observed months ago is not today's state, and the axis
                // says so beside the count it contributed to.
                "aging": report
                    .anchors
                    .iter()
                    .filter(|a| a.observed_at.is_some())
                    .map(|a| {
                        let days = a.unobserved_for_days.unwrap_or(0);
                        serde_json::json!({
                            "id": a.entity_id,
                            "artifact": a.artifact,
                            "state": a.state,
                            "observed_at": a.observed_at,
                            "unobserved_for_days": days,
                            "note": format!("unobserved for {days} days"),
                        })
                    })
                    .collect::<Vec<_>>(),
        });
        if let (Some(o), Ok(serde_json::Value::Object(f))) =
            (row.as_object_mut(), serde_json::to_value(&report.figure))
        {
            o.extend(f);
        }
        out.insert(mem, row);
    }
    serde_json::Value::Object(out)
}

/// Compute health reports for all entities in the store.
///
/// `mem_schemas` maps mem name → `Arc<Schema>`. Entities whose mem
/// is missing from this map fall back to the builtin `default` schema
/// relationship vocabulary (keeps legacy fixtures green; real production
/// paths always register a mem schema).
///
/// `mem_filter` scopes the per-entity scans and the structural counts
/// (orphans, stubs, leaf population) to one mem; `None` is the classic
/// engine-wide sweep. Validating that the name exists is the caller's
/// job ([`crate::Engine::health_scoped`] refuses `UNKNOWN_MEM` before
/// reaching here) — an unknown name at this level just scans nothing.
pub fn compute_health(
    store: &Store,
    default_schema: &TypeDefinition,
    mem_schemas: &HashMap<String, Arc<Schema>>,
    mem_filter: Option<&str>,
) -> HealthSummary {
    let mut missing_fields = Vec::new();
    let mut stale_entities = Vec::new();

    let today_days = days_since_epoch();

    let in_scope = |mem: &str| mem_filter.is_none_or(|v| mem == v);

    for entity in store.all_entities() {
        if entity.stub || !in_scope(&entity.mem) {
            continue;
        }

        // Resolve the entity's `TypeDefinition` against the entity's
        // own mem's schema first. `type_by_name` only knows the
        // builtin `default` schema; falling through to it on a mem
        // pinned to a non-default schema (e.g. `planning@0.1.0`) would
        // silently use `default_schema` (effectively `spec`) for every
        // entity and report `spec`'s `health_required_fields` —
        // `[identity, purpose]` — even on entities of types like
        // `goal` / `option` / `decision`.
        let resolved = mem_schemas
            .get(entity.mem.as_str())
            .and_then(|s| s.types.get(entity.entity_type.as_str()).cloned())
            .or_else(|| type_by_name(&entity.entity_type));
        let schema: &TypeDefinition = resolved.as_deref().unwrap_or(default_schema);
        let mut issues = Vec::new();

        // Check health_required_fields
        for field in &schema.health_required_fields {
            // Check if it's a section or metadata field
            if schema.section(field).is_some() {
                // It's a section. When the content is present in the
                // file but sits under a non-deriving heading, report
                // the distinct mismatch finding instead of "missing" —
                // the two conditions must never collapse.
                let content = entity.sections.get(field.as_str());
                if content.is_none_or(|c| c.trim().is_empty()) {
                    if let Some(issue) = section_heading_mismatch_issue(entity, schema, field) {
                        issues.push(issue);
                    } else {
                        issues.push(HealthIssue {
                            field: field.clone(),
                            code: super::HealthIssueCode::Missing,
                            message: format!("required section '{field}' is empty"),
                        });
                    }
                }
            } else {
                // It's a metadata field. Treat missing AND empty /
                // whitespace-only values as gaps so the scan matches
                // the section branch's `trim().is_empty()` semantics
                // — an empty `MetadataValue::String("")` is just as
                // unhelpful to an agent as an absent key.
                let value = entity.metadata.get(field.as_str());
                let is_empty = match value {
                    None => true,
                    Some(v) => v.to_frontmatter_string().trim().is_empty(),
                };
                if is_empty {
                    issues.push(HealthIssue {
                        field: field.clone(),
                        code: super::HealthIssueCode::Missing,
                        message: format!("required field '{field}' is missing"),
                    });
                }
            }
        }

        // The heading-mismatch condition is drift worth surfacing on
        // every declared section, not only the health-required ones.
        for s in schema.sections.iter().filter(|s| !s.catch_all) {
            if schema.health_required_fields.contains(&s.key) {
                continue; // already handled above
            }
            let content = entity.sections.get(s.key.as_str());
            if content.is_none_or(|c| c.trim().is_empty())
                && let Some(issue) = section_heading_mismatch_issue(entity, schema, &s.key)
            {
                issues.push(issue);
            }
        }

        // Undeclared-relationship warning. Scan the entity's
        // relationship list against the mem's schema vocabulary; every
        // unknown name becomes a soft HealthIssue (same severity as a
        // missing section) so agents running a health sweep after a
        // schema version bump see drift without a crashed load.
        //
        // Shape-violation scan: when the mem's schema declares
        // `source_types` / `target_types` on a relationship and an
        // existing edge violates the shape, surface as a soft
        // HealthIssue. The relate-add path enforces shape going
        // forward; this scan catches edges authored before the
        // constraint landed (or via inline `relations:` on
        // memstead_create, which does not yet shape-check). The
        // remove-path on `memstead_relate` skips shape validation so the
        // cleanup is always reachable.
        if let Some(mem_schema) = mem_schemas.get(entity.mem.as_str()) {
            let mut seen_unknown = std::collections::HashSet::new();
            for rel in &entity.relationships {
                if !mem_schema.relationship_known(&rel.rel_type) {
                    if seen_unknown.insert(rel.rel_type.clone()) {
                        let suggestion = mem_schema
                            .suggest_relationship(&rel.rel_type)
                            .map(|s| format!(" Did you mean '{s}'?"))
                            .unwrap_or_default();
                        let (schema_name, schema_version) = mem_schema.id();
                        issues.push(HealthIssue {
                            field: "relationships".to_string(),
                            code: super::HealthIssueCode::UndeclaredRelationship,
                            message: format!(
                                "relationship '{}' is not declared in schema \
                                 '{schema_name}@{schema_version}'.{suggestion}",
                                rel.rel_type
                            ),
                        });
                    }
                    continue;
                }

                let target_type = store
                    .get(&rel.target)
                    .map(|t| t.entity_type.clone())
                    .filter(|t| !t.is_empty());
                if let Err(crate::runtime_validator::ValidationError::InvalidRelationshipShape {
                    rel_type,
                    from_type,
                    to_type,
                    allowed_source_types,
                    allowed_target_types,
                    ..
                }) = crate::runtime_validator::validate_rel_shape(
                    &rel.rel_type,
                    entity.entity_type.as_str(),
                    target_type.as_deref(),
                    mem_schema.as_ref(),
                ) {
                    let allowed_src = if allowed_source_types.is_empty() {
                        "<any>".to_string()
                    } else {
                        allowed_source_types.join(", ")
                    };
                    let allowed_tgt = if allowed_target_types.is_empty() {
                        "<any>".to_string()
                    } else {
                        allowed_target_types.join(", ")
                    };
                    issues.push(HealthIssue {
                        field: "relationships".to_string(),
                        code: super::HealthIssueCode::InvalidRelShape,
                        message: format!(
                            "INVALID_REL_SHAPE: edge '{rel_type}' from \
                             '{from_type}' to '{to_type}' (target {target}) \
                             violates declared shape — allowed_source_types: \
                             [{allowed_src}], allowed_target_types: \
                             [{allowed_tgt}]. Remove via \
                             `memstead_relate from={from_id} to={target} \
                             type={rel_type} remove=true`.",
                            target = rel.target,
                            from_id = entity.id,
                        ),
                    });
                }
            }
        }

        // Staleness check
        let auto_ts_field = schema.metadata_fields.iter().find(|f| f.auto_timestamp);

        if let Some(ts_field) = auto_ts_field
            && let Some(val) = entity.metadata.get(ts_field.key.as_str())
        {
            let date_str = val.to_frontmatter_string();
            if let Some(modified_days) = parse_iso_to_days(&date_str) {
                let days_since = today_days.saturating_sub(modified_days);
                if days_since > schema.staleness_threshold_days as u64 {
                    stale_entities.push(StaleEntity {
                        id: entity.id.clone(),
                        title: entity.title.clone(),
                        days_since_modified: days_since,
                        anchor_state: None,
                    });
                }
            }
        }

        if !issues.is_empty() {
            // Compute a simple health score: (total_fields - issues) / total_fields.
            // `issues.len()` can exceed `total_fields` once the
            // relationship-vocabulary issues are added on top, so saturate
            // the subtraction rather than underflow. A score of 0.0 is the
            // natural floor — agents treat it as "maximally broken".
            let total = schema.health_required_fields.len();
            let score = if total > 0 {
                (total.saturating_sub(issues.len()) as f32) / (total as f32)
            } else {
                1.0
            };

            missing_fields.push(HealthReport {
                id: entity.id.clone(),
                title: entity.title.clone(),
                score,
                issues,
            });
        }
    }

    // Sort stale entities by days_since_modified descending
    stale_entities.sort_by_key(|e| std::cmp::Reverse(e.days_since_modified));

    // Structural counts — scoped by the same filter as the entity scans
    // above so a `mem`-scoped summary is internally consistent.
    let orphan_count = query::find_orphans_with_schemas(store, mem_schemas)
        .into_iter()
        .filter(|id| store.get(id).is_some_and(|e| in_scope(&e.mem)))
        .count();
    let leaf_entities_by_type = match mem_filter {
        None => query::leaf_population(store, mem_schemas),
        Some(v) => {
            let scoped: HashMap<String, Arc<Schema>> = mem_schemas
                .iter()
                .filter(|(mem, _)| mem.as_str() == v)
                .map(|(mem, s)| (mem.clone(), s.clone()))
                .collect();
            query::leaf_population(store, &scoped)
        }
    };
    let stub_count = query::find_stubs(store)
        .iter()
        .filter(|(id, _)| store.get(id).is_some_and(|e| in_scope(&e.mem)))
        .count();

    // The store iterates a hash map: order the per-entity lists by id so
    // two processes (the CLI and the MCP server) render the same bytes.
    stale_entities.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    missing_fields.sort_by(|a, b| a.id.0.cmp(&b.id.0));

    HealthSummary {
        stale_entities,
        anchor_fresh: Vec::new(),
        missing_fields,
        orphan_count,
        stub_count,
        warnings: Vec::new(),
        quarantined: Vec::new(),
        load_errors: Vec::new(),
        boot_diagnosis: None,
        leaf_entities_by_type,
        dangling_links: None,
        findings: None,
        tag_distribution: None,
        tag_distribution_folded: None,
        untagged_entities: None,
    }
}

/// Scan every non-stub entity's `tags` metadata and aggregate (tag → count,
/// per-entity-type breakdown) plus untagged coverage. Comma-separated parser
/// with per-segment trim; empty segments drop. Comparison is case-sensitive
/// on the primary surface — case drift is surfaced separately via
/// [`TagDistribution`] siblings folded by the caller if desired.
///
/// `mem_filter` narrows both aggregation passes to entities in that mem;
/// `limit` caps the returned `tag_distribution` array after sorting by count
/// descending (tie-break by tag ascending for deterministic output).
///
/// Also returns `FoldedTag` entries for any canonical (lowercase) tag where
/// two or more authored casings appear — drift-flag only; empty when no
/// collisions exist.
pub fn collect_tag_distribution(
    store: &Store,
    mem_filter: Option<&str>,
    limit: usize,
) -> (Vec<TagDistribution>, Vec<FoldedTag>, UntaggedStats) {
    // tag → (count, per_type_count)
    let mut counts: HashMap<String, (usize, HashMap<String, usize>)> = HashMap::new();
    let mut untagged = UntaggedStats {
        total: 0,
        by_entity_type: HashMap::new(),
    };

    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        if let Some(v) = mem_filter
            && entity.mem != v
        {
            continue;
        }

        let tags_raw = entity
            .metadata
            .get("tags")
            .and_then(|v| match v {
                MetadataValue::String(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");

        let mut any_tag = false;
        for tag in tags_raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            any_tag = true;
            let entry = counts
                .entry(tag.to_string())
                .or_insert_with(|| (0, HashMap::new()));
            entry.0 += 1;
            *entry.1.entry(entity.entity_type.clone()).or_insert(0) += 1;
        }
        if !any_tag {
            untagged.total += 1;
            *untagged
                .by_entity_type
                .entry(entity.entity_type.clone())
                .or_insert(0) += 1;
        }
    }

    // Primary distribution — case-sensitive.
    let mut entries: Vec<TagDistribution> = counts
        .iter()
        .map(|(tag, (count, by_type))| TagDistribution {
            tag: tag.clone(),
            count: *count,
            by_entity_type: by_type.clone(),
        })
        .collect();
    entries.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    entries.truncate(limit);

    // Case-drift sidecar: group by lowercase canonical; surface only entries
    // with ≥2 distinct authored casings. Operates on the full counts map, not
    // the truncated primary surface, so drift hidden below `limit` still
    // surfaces.
    let mut by_canonical: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    for (tag, (count, _)) in counts.iter() {
        by_canonical
            .entry(tag.to_lowercase())
            .or_default()
            .push((tag.clone(), *count));
    }
    let mut folded: Vec<FoldedTag> = by_canonical
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(canonical, mut variants)| {
            variants.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let total = variants.iter().map(|(_, c)| *c).sum();
            FoldedTag {
                canonical,
                total,
                variants: variants
                    .into_iter()
                    .map(|(tag, count)| TagVariant { tag, count })
                    .collect(),
            }
        })
        .collect();
    folded.sort_by(|a, b| {
        b.total
            .cmp(&a.total)
            .then_with(|| a.canonical.cmp(&b.canonical))
    });

    (entries, folded, untagged)
}

/// Collect the three separately-repaired conditions the diagnostic
/// covers, each tagged with its own [`DanglingLinkKind`]:
///
/// - `LinkTargetMissing` — a body wiki-link whose target has no markdown
///   file: either absent from the store entirely, or present only as a
///   stub. Repair: write the entity, or drop the link.
/// - `LinkNotRelated` — a body wiki-link to a fully written entity that
///   the referrer's relationships list omits (an alias orphan under the
///   alias model). The target is fine; the row is missing. Repair: a
///   write of the referrer re-synthesises the row.
/// - `RelationTargetMissing` — a `## Relationships` row naming an entity
///   absent from the store entirely, not even as a stub. Auto-stubbing
///   means this only arises from out-of-band file edits or historical
///   cross-mem corruption (the pre-F15 mem-delete path). Repair: restore
///   the target or remove the edge.
///
/// The three used to share one code, and only the first was identifiable
/// from the payload; the other two were told apart, if at all, by
/// whether `section` happened to be null. `section` still marks the
/// source axis (`None` for the relationship table), but the condition is
/// named, not inferred.
///
/// `mem_filter` narrows *scanning* to entities in that mem; resolution
/// stays global so cross-mem links whose target is a real entity
/// elsewhere are not flagged as missing.
pub fn collect_dangling_links(store: &Store, mem_filter: Option<&str>) -> Vec<DanglingLink> {
    use crate::entity::parser::extract_inline_links_lenient;
    use std::collections::HashSet;

    let mut out = Vec::new();
    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        if let Some(v) = mem_filter
            && entity.mem != v
        {
            continue;
        }
        let explicit_targets: HashSet<_> = entity
            .relationships
            .iter()
            .map(|r| r.target.clone())
            .collect();
        for (section_key, section_body) in &entity.sections {
            for target_id in extract_inline_links_lenient(section_body, &entity.mem) {
                let target_missing = store.get(&target_id).map(|e| e.stub).unwrap_or(true);
                let alias_orphan = !target_missing && !explicit_targets.contains(&target_id);
                // The one place the three conditions are distinguished, so the
                // one place the discriminator is set. Consumers read `kind`;
                // none re-derives it against the store (04/06).
                let kind = if target_missing {
                    crate::ops::DanglingLinkKind::LinkTargetMissing
                } else {
                    crate::ops::DanglingLinkKind::LinkNotRelated
                };
                if target_missing || alias_orphan {
                    out.push(DanglingLink {
                        kind,
                        from: entity.id.clone(),
                        target_id: target_id.clone(),
                        target_path: target_id.path().to_string(),
                        section: Some(section_key.clone()),
                    });
                }
            }
        }
        // Relationship-table dangler scan. The `## Relationships`
        // section is structurally distinct from body sections — its
        // rows materialise from `entity.relationships` rather than a
        // free-text body — so `section: None` marks the source axis.
        //
        // Discrimination differs from the body scan: a relationship
        // target that resolves to a stub is a legitimate forward
        // reference (the alias machinery auto-stubs absent targets
        // by design), not corruption. Only a target that's *fully
        // absent* from the store — neither stub nor real — flags as
        // dangling. In practice this only fires for out-of-band file
        // edits or historical cross-mem-delete corruption that
        // dropped the stub along with the deleted mem.
        //
        // Dedup against the body-scan output so a target that
        // surfaces from both axes doesn't double-emit.
        for rel in &entity.relationships {
            if store.get(&rel.target).is_some() {
                continue;
            }
            let already_reported = out
                .iter()
                .any(|d| d.from == entity.id && d.target_id == rel.target);
            if already_reported {
                continue;
            }
            out.push(DanglingLink {
                kind: crate::ops::DanglingLinkKind::RelationTargetMissing,
                from: entity.id.clone(),
                target_id: rel.target.clone(),
                target_path: rel.target.path().to_string(),
                section: None,
            });
        }
    }
    // Deterministic output — the store iterates a HashMap, so without a
    // sort two identical runs can serve the same findings in different
    // orders. Sort by (from, target, section) so successive sweeps diff
    // cleanly.
    out.sort_by(|a, b| {
        (&a.from.0, &a.target_id.0, &a.section).cmp(&(&b.from.0, &b.target_id.0, &b.section))
    });
    out
}

/// Collect every non-stub entity whose type declares `required_outgoing`
/// blocks that the entity's current outgoing edges leave unsatisfied.
/// Results are deterministic — sorted
/// by `(mem, id)` — so the agent can diff successive sweeps without
/// the underlying HashMap iteration order leaking through.
///
/// `mem_filter` narrows scanning to entities in that mem when set;
/// `mem_schemas` resolves the entity's type definition against the
/// mem's pinned schema. Entities whose mem has no schema in the
/// map are skipped (no schema → no `required_outgoing` to evaluate).
pub fn collect_missing_required_outgoing(
    store: &Store,
    mem_filter: Option<&str>,
    mem_schemas: &HashMap<String, Arc<memstead_schema::Schema>>,
) -> Vec<MissingRequiredOutgoingReport> {
    let mut out = Vec::new();
    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        if let Some(v) = mem_filter
            && entity.mem != v
        {
            continue;
        }
        let Some(mem_schema) = mem_schemas.get(entity.mem.as_str()) else {
            continue;
        };
        let Some(td) = mem_schema.types.get(entity.entity_type.as_str()) else {
            continue;
        };
        if td.required_outgoing.is_empty() {
            continue;
        }
        let unsatisfied = unsatisfied_required_outgoing(entity, td);
        if unsatisfied.is_empty() {
            continue;
        }
        out.push(MissingRequiredOutgoingReport {
            id: entity.id.clone(),
            title: entity.title.clone(),
            entity_type: entity.entity_type.clone(),
            mem: entity.mem.clone(),
            missing: unsatisfied,
        });
    }
    out.sort_by(|a, b| a.mem.cmp(&b.mem).then_with(|| a.id.0.cmp(&b.id.0)));
    out
}

/// Evaluate one entity's declared `required_outgoing` blocks against
/// its current outgoing edges, returning the unsatisfied blocks in
/// declaration order. THE single evaluation — shared by the health
/// sweep ([`collect_missing_required_outgoing`]) and the per-mutation
/// `MISSING_REQUIRED_OUTGOING` warning on create/update. A second
/// implementation of the block check is a defect: the two surfaces
/// must never disagree about what counts as unsatisfied.
pub fn unsatisfied_required_outgoing(
    entity: &crate::entity::Entity,
    td: &TypeDefinition,
) -> Vec<super::MissingRequiredOutgoingBlock> {
    td.required_outgoing
        .iter()
        .filter(|block| {
            // A conditional block applies only while `when_field`
            // holds `when_value` (same comparison the `requires_when`
            // constraint uses). Unset field or any other value = the
            // block is unarmed and never unsatisfied.
            if let (Some(when_field), Some(when_value)) = (&block.when_field, &block.when_value) {
                let armed = entity
                    .metadata
                    .get(when_field.as_str())
                    .is_some_and(|v| v.to_frontmatter_string() == *when_value);
                if !armed {
                    return false;
                }
            }
            let count = entity
                .relationships
                .iter()
                .filter(|rel| block.relationships.iter().any(|name| name == &rel.rel_type))
                .count();
            !block.admits(count)
        })
        .map(|block| super::MissingRequiredOutgoingBlock {
            relationships: block.relationships.clone(),
            cardinality: block.cardinality.to_string(),
            severity: block.severity,
            when_field: block.when_field.clone(),
            when_value: block.when_value.clone(),
        })
        .collect()
}

/// One violated declared constraint on one entity — the wire entry
/// shared by the write-path surface (the `CONSTRAINT_UNSATISFIED`
/// warning or refusal, tier decided by the declared severity) and the
/// health `constraints` include. The serde `kind` tag names the form;
/// the remaining fields restate the declaration (plus the observed
/// offense — the colliding entity, the unbacked value, the tainting
/// ancestor) so a consumer can repair without re-fetching the schema.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnsatisfiedConstraint {
    RequiresWhen {
        field: String,
        when_field: String,
        when_value: String,
        severity: memstead_schema::ConstraintSeverity,
    },
    Unique {
        fields: Vec<String>,
        /// The entity's values for `fields`, in declaration order.
        values: Vec<String>,
        /// The other entity holding the same tuple (lexically smallest
        /// when several collide).
        colliding: String,
        severity: memstead_schema::ConstraintSeverity,
    },
    EnumFromNeighbour {
        field: String,
        /// The set value no reached neighbour's section backs.
        value: String,
        rel_type: String,
        section: String,
        severity: memstead_schema::ConstraintSeverity,
    },
    StatusPropagation {
        field: String,
        /// The terminal value the ancestor holds.
        value: String,
        /// Echo of a single-rel-type declaration — present exactly
        /// when the schema declared `rel_type`, keeping the
        /// long-standing payload byte-identical.
        #[serde(skip_serializing_if = "Option::is_none")]
        rel_type: Option<String>,
        /// Echo of a relation-set declaration (`rel_types`).
        #[serde(skip_serializing_if = "Option::is_none")]
        rel_types: Option<Vec<String>>,
        /// The tainting ancestor — the entity holding the terminal
        /// value that this entity (transitively) reaches.
        tainted_by: String,
        severity: memstead_schema::ConstraintSeverity,
    },
    /// Form 6 — the entity holds the gated `to_value` while related
    /// entities lack a fresh confirming check record.
    TransitionRequiresChecks {
        field: String,
        to_value: String,
        relationships: Vec<String>,
        direction: memstead_schema::PropagationDirection,
        /// Every related entity NOT at derived state `checked_ok`,
        /// each with the state it derived, sorted by id.
        unchecked: Vec<UncheckedRelated>,
        /// How many related entities the declared edges reach.
        related: usize,
        /// The declared floor; a violation with `related < min_related`
        /// is the vacuous case the floor exists to refuse.
        min_related: usize,
        severity: memstead_schema::ConstraintSeverity,
    },
    /// Form 7 — the entity holds the gated `to_value` while it carries
    /// no fresh, independent, confirming check record of `check_kind`.
    TransitionRequiresSelfCheck {
        field: String,
        to_value: String,
        check_kind: String,
        /// The standing the entity's own record derived instead of an
        /// independent `checked_ok` (`never_checked`, `check_stale`,
        /// `check_failed`, `self_checked`, `unconfirmable`).
        state: String,
        severity: memstead_schema::ConstraintSeverity,
    },
    /// The entity reaches no non-stub entity of a terminal type along
    /// the declared relation set — the declaration is echoed whole so
    /// the reader sees which obligation went unmet without re-fetching
    /// the schema. Health-sweep only, always warn-tier.
    MustReach {
        relationships: Vec<String>,
        direction: memstead_schema::ReachDirection,
        terminal_types: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_depth: Option<u32>,
        severity: memstead_schema::ConstraintSeverity,
    },
}

impl UnsatisfiedConstraint {
    pub fn severity(&self) -> memstead_schema::ConstraintSeverity {
        match self {
            Self::RequiresWhen { severity, .. }
            | Self::Unique { severity, .. }
            | Self::EnumFromNeighbour { severity, .. }
            | Self::StatusPropagation { severity, .. }
            | Self::MustReach { severity, .. }
            | Self::TransitionRequiresChecks { severity, .. }
            | Self::TransitionRequiresSelfCheck { severity, .. } => *severity,
        }
    }

    /// One-line human rendering for warning/refusal message text.
    pub fn describe(&self) -> String {
        match self {
            Self::RequiresWhen {
                field,
                when_field,
                when_value,
                ..
            } => format!(
                "requires_when: '{field}' is required when {when_field}={when_value} and is unset"
            ),
            Self::Unique {
                fields, colliding, ..
            } => format!(
                "unique: tuple ({}) collides with '{colliding}'",
                fields.join(", ")
            ),
            Self::EnumFromNeighbour {
                field,
                value,
                rel_type,
                section,
                ..
            } => format!(
                "enum_from_neighbour: '{field}' value '{value}' has no backing entry in any \
                 `{section}` section reached via {rel_type}"
            ),
            Self::StatusPropagation {
                field,
                value,
                tainted_by,
                ..
            } => {
                format!("status_propagation: tainted by '{tainted_by}' ({field}={value})")
            }
            Self::MustReach {
                relationships,
                direction,
                terminal_types,
                max_depth,
                ..
            } => {
                let depth = match max_depth {
                    Some(d) => format!(" within {d} hop(s)"),
                    None => String::new(),
                };
                format!(
                    "must_reach: no path via [{}] ({direction}) reaches a [{}] entity{depth}",
                    relationships.join(", "),
                    terminal_types.join(", ")
                )
            }
            Self::TransitionRequiresChecks {
                field,
                to_value,
                relationships,
                unchecked,
                related,
                min_related,
                ..
            } => {
                if related < min_related {
                    return format!(
                        "transition_requires_checks: {field}={to_value} requires at least \
                         {min_related} related entit{} via [{}] — found {related}",
                        if *min_related == 1 { "y" } else { "ies" },
                        relationships.join(", ")
                    );
                }
                let listed: Vec<String> = unchecked
                    .iter()
                    .map(|u| format!("'{}' ({})", u.id, u.state))
                    .collect();
                format!(
                    "transition_requires_checks: {field}={to_value} requires a fresh confirming \
                     check record on every entity related via [{}] — unconfirmed: {}",
                    relationships.join(", "),
                    listed.join(", ")
                )
            }
            Self::TransitionRequiresSelfCheck {
                field,
                to_value,
                check_kind,
                state,
                ..
            } => format!(
                "transition_requires_self_check: {field}={to_value} requires a fresh confirming \
                 `{check_kind}` check record on this entity under an identity other than its \
                 author — derived: {state}"
            ),
        }
    }
}

/// One related entity blocking a `transition_requires_checks` gate:
/// its id and the derived verification state it reads instead of the
/// required `checked_ok`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UncheckedRelated {
    pub id: String,
    pub state: String,
}

/// The gated-transition evaluators' window into the check ledger:
/// the derived standing of one entity's newest check record of one
/// wire kind (`verification`, `conformance`, or a foreign `x-<name>`).
/// Callers with an engine build it from the workspace check ledger;
/// `None` (no ledger access — a workspace-less engine, or a call path
/// that cannot reach one) derives every entity as `never_checked`, so
/// a declared gate refuses honestly rather than passing unverified.
pub type CheckStateProvider<'a> =
    &'a dyn Fn(&crate::entity::Entity, &str) -> crate::engine::independence::CheckStanding;

/// Evaluate one entity's declared per-entity `constraints` against its
/// current state (and, for the store-aware forms, against the rest of
/// its mem), returning the violated ones in declaration order. THE
/// single evaluation — shared by the health sweep
/// ([`collect_constraint_findings`]) and the per-mutation
/// `CONSTRAINT_UNSATISFIED` surface on create/update/relate; a second
/// implementation of any form is a defect.
///
/// Form semantics:
/// - `requires_when` triggers when `when_field`'s frontmatter value
///   equals `when_value` exactly; a triggered constraint is satisfied
///   when `field` — a metadata field or a section key — is present
///   with non-blank content.
/// - `unique`: the entity's tuple of `fields` values (skipped when any
///   field is unset/blank) must not equal another non-stub entity's
///   tuple within the same mem and type. `exclude` names the entity's
///   own id so an update does not collide with its stored self.
/// - `enum_from_neighbour`: a set `field` value must appear as a
///   bullet entry (`- value` / `* value` line) in the `section` body
///   of at least one entity reached via an outgoing `rel_type` edge.
/// - `status_propagation` is a reachability property of the graph,
///   not of one write — it is evaluated only by the health sweep
///   ([`collect_constraint_findings`]), never here.
pub fn unsatisfied_constraints(
    store: &Store,
    entity: &crate::entity::Entity,
    td: &TypeDefinition,
    exclude: Option<&crate::entity::EntityId>,
    checks: Option<CheckStateProvider<'_>>,
) -> Vec<UnsatisfiedConstraint> {
    use memstead_schema::ConstraintDef;
    td.constraints
        .iter()
        .filter_map(|c| match c {
            ConstraintDef::RequiresWhen {
                field,
                when_field,
                when_value,
                severity,
            } => {
                let triggered = entity
                    .metadata
                    .get(when_field.as_str())
                    .is_some_and(|v| v.to_frontmatter_string() == *when_value);
                if !triggered {
                    return None;
                }
                let satisfied = entity
                    .metadata
                    .get(field.as_str())
                    .is_some_and(|v| !v.to_frontmatter_string().trim().is_empty())
                    || entity
                        .sections
                        .get(field.as_str())
                        .is_some_and(|body| !body.trim().is_empty());
                if satisfied {
                    return None;
                }
                Some(UnsatisfiedConstraint::RequiresWhen {
                    field: field.clone(),
                    when_field: when_field.clone(),
                    when_value: when_value.clone(),
                    severity: *severity,
                })
            }
            ConstraintDef::Unique { fields, severity } => {
                let tuple = tuple_of(entity, fields)?;
                let mut colliding: Vec<&str> = store
                    .all_entities()
                    .filter(|other| {
                        !other.stub
                            && other.mem == entity.mem
                            && other.entity_type == entity.entity_type
                            && Some(&other.id) != exclude
                            && other.id != entity.id
                            && tuple_of(other, fields).as_ref() == Some(&tuple)
                    })
                    .map(|other| other.id.0.as_str())
                    .collect();
                colliding.sort_unstable();
                let first = colliding.first()?;
                Some(UnsatisfiedConstraint::Unique {
                    fields: fields.clone(),
                    values: tuple,
                    colliding: first.to_string(),
                    severity: *severity,
                })
            }
            ConstraintDef::EnumFromNeighbour {
                field,
                rel_type,
                section,
                severity,
            } => {
                let value = entity
                    .metadata
                    .get(field.as_str())
                    .map(|v| v.to_frontmatter_string())
                    .filter(|v| !v.trim().is_empty())?;
                let backed = entity
                    .relationships
                    .iter()
                    .filter(|rel| rel.rel_type == *rel_type)
                    .filter_map(|rel| store.get(&rel.target))
                    .filter_map(|neighbour| neighbour.sections.get(section.as_str()))
                    .any(|body| bullet_entries(body).contains(&value));
                if backed {
                    return None;
                }
                Some(UnsatisfiedConstraint::EnumFromNeighbour {
                    field: field.clone(),
                    value,
                    rel_type: rel_type.clone(),
                    section: section.clone(),
                    severity: *severity,
                })
            }
            ConstraintDef::StatusPropagation { .. } => None,
            ConstraintDef::TransitionRequiresChecks {
                field,
                to_value,
                relationships,
                direction,
                min_related,
                severity,
            } => {
                let triggered = entity
                    .metadata
                    .get(field.as_str())
                    .is_some_and(|v| v.to_frontmatter_string() == *to_value);
                if !triggered {
                    return None;
                }
                let (related, unchecked) = transition_gate_standing(
                    store,
                    entity,
                    relationships,
                    *direction,
                    exclude,
                    checks,
                );
                if unchecked.is_empty() && related >= *min_related {
                    return None;
                }
                Some(UnsatisfiedConstraint::TransitionRequiresChecks {
                    field: field.clone(),
                    to_value: to_value.clone(),
                    relationships: relationships.clone(),
                    direction: *direction,
                    unchecked,
                    related,
                    min_related: *min_related,
                    severity: *severity,
                })
            }
            ConstraintDef::TransitionRequiresSelfCheck {
                field,
                to_value,
                check_kind,
                severity,
            } => {
                let triggered = entity
                    .metadata
                    .get(field.as_str())
                    .is_some_and(|v| v.to_frontmatter_string() == *to_value);
                if !triggered {
                    return None;
                }
                // The entity's own record of the declared kind, read
                // through the same provider form 6 reads related
                // entities with: no ledger derives never_checked, the
                // author's own ok reads self_checked, neither confirms.
                let standing = match checks {
                    Some(provider) => provider(entity, check_kind),
                    None => crate::engine::independence::CheckStanding {
                        state: crate::check::CheckState::NeverChecked,
                        independence: None,
                    },
                };
                if standing.confirms() {
                    return None;
                }
                Some(UnsatisfiedConstraint::TransitionRequiresSelfCheck {
                    field: field.clone(),
                    to_value: to_value.clone(),
                    check_kind: check_kind.clone(),
                    state: standing.label().to_string(),
                    severity: *severity,
                })
            }
        })
        .collect()
}

/// The standing of one gated-transition constraint's related set
/// against one entity, independent of whether the entity currently
/// holds the gated value: `(total related, those lacking a fresh
/// confirming check record)`, the unconfirmed sorted by id. THE single
/// related-set enumeration — shared by the write-time evaluator arm
/// above and the gates-brief renderer, so the brief can never disagree
/// with the refusal. Outgoing reads the entity's own edges (its
/// written state); incoming scans the store for edge sources pointing
/// at it, excluding the entity's own stored copy (`exclude`) so an
/// update never gates on its pre-write self.
pub fn transition_gate_standing(
    store: &Store,
    entity: &crate::entity::Entity,
    relationships: &[String],
    direction: memstead_schema::PropagationDirection,
    exclude: Option<&crate::entity::EntityId>,
    checks: Option<CheckStateProvider<'_>>,
) -> (usize, Vec<UncheckedRelated>) {
    let related: Vec<&crate::entity::Entity> = match direction {
        memstead_schema::PropagationDirection::Outgoing => entity
            .relationships
            .iter()
            .filter(|rel| relationships.contains(&rel.rel_type))
            .filter_map(|rel| store.get(&rel.target))
            .collect(),
        memstead_schema::PropagationDirection::Incoming => store
            .all_entities()
            .filter(|other| {
                other.id != entity.id
                    && Some(&other.id) != exclude
                    && other
                        .relationships
                        .iter()
                        .any(|rel| rel.target == entity.id && relationships.contains(&rel.rel_type))
            })
            .collect(),
    };
    let total = related.len();
    let mut unchecked: Vec<UncheckedRelated> = related
        .into_iter()
        .filter_map(|rel_entity| {
            // An ok check confirms only when it is independent of the
            // executors (engine::independence): the executor's own ok
            // reads `self_checked` here and does not close the gate.
            let standing = match checks {
                Some(provider) => {
                    provider(rel_entity, crate::check::CheckKind::Verification.as_str())
                }
                None => crate::engine::independence::CheckStanding {
                    state: crate::check::CheckState::NeverChecked,
                    independence: None,
                },
            };
            if standing.confirms() {
                None
            } else {
                Some(UncheckedRelated {
                    id: rel_entity.id.0.clone(),
                    state: standing.label().to_string(),
                })
            }
        })
        .collect();
    unchecked.sort_by(|a, b| a.id.cmp(&b.id));
    (total, unchecked)
}

/// The entity's tuple of frontmatter values for `fields`, in
/// declaration order — `None` when any field is unset or blank (no
/// tuple, nothing to compare).
fn tuple_of(entity: &crate::entity::Entity, fields: &[String]) -> Option<Vec<String>> {
    fields
        .iter()
        .map(|f| {
            entity
                .metadata
                .get(f.as_str())
                .map(|v| v.to_frontmatter_string())
                .filter(|v| !v.trim().is_empty())
        })
        .collect()
}

/// The bullet entries of a section body — trimmed text of `- item` /
/// `* item` lines. The legal-value shape `enum_from_neighbour` reads.
fn bullet_entries(body: &str) -> Vec<String> {
    // A bullet inside a code block is an example of the list, not a
    // member of it — the same referee every other content reader uses
    // ([`crate::markdown`]). Masking preserves byte offsets and line
    // count, so each masked line pairs with its original.
    let masked = crate::markdown::mask_code_blocks_and_spans(body);
    body.lines()
        .zip(masked.lines())
        .filter_map(|(line, masked_line)| {
            let m = masked_line.trim_start();
            if m.starts_with("- ") || m.starts_with("* ") {
                let t = line.trim_start();
                t.strip_prefix("- ")
                    .or_else(|| t.strip_prefix("* "))
                    .map(|e| e.trim().to_string())
            } else {
                None
            }
        })
        .collect()
}

/// One entity's violated declared constraints, surfaced from the
/// health-time scan (`include=["constraints"]`). Mirrors
/// [`MissingRequiredOutgoingReport`]'s envelope shape — the two
/// includes read the same way.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConstraintFindingReport {
    pub id: crate::entity::EntityId,
    pub title: String,
    pub entity_type: String,
    pub mem: String,
    pub violations: Vec<UnsatisfiedConstraint>,
    /// Standing violations of the entity's declared section formats
    /// (plan 08) — additive: consumers of the pre-format shape see an
    /// absent key, never an empty list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub format_violations: Vec<crate::section_format::SectionFormatViolation>,
}

/// Collect every non-stub entity whose declared `constraints` its
/// current state violates. Two passes: the per-entity forms
/// (`requires_when`, `unique`, `enum_from_neighbour`) through the
/// shared [`unsatisfied_constraints`] evaluation, then the
/// `status_propagation` graph sweep — for each entity holding a
/// declared terminal value, every entity reaching it (transitively)
/// via the declared rel-type and direction gains a finding naming that
/// tainting ancestor. Deterministic — reports sorted by `(mem, id)`,
/// violations in declaration order then by tainting ancestor.
pub fn collect_constraint_findings(
    store: &Store,
    mem_filter: Option<&str>,
    mem_schemas: &HashMap<String, Arc<memstead_schema::Schema>>,
    checks: Option<CheckStateProvider<'_>>,
) -> Vec<ConstraintFindingReport> {
    use memstead_schema::ConstraintDef;
    type Bucket = (
        Vec<UnsatisfiedConstraint>,
        Vec<crate::section_format::SectionFormatViolation>,
    );
    let mut by_entity: std::collections::BTreeMap<String, Bucket> = Default::default();

    // Reverse adjacency for `must_reach` incoming walks — built once
    // per sweep, and only when some pinned schema declares one (a
    // workspace without the form pays nothing).
    let needs_reverse = mem_schemas.values().any(|s| {
        s.types.values().any(|t| {
            t.must_reach
                .iter()
                .any(|ob| ob.direction == memstead_schema::ReachDirection::In)
        })
    });
    let reverse: ReverseIndex = if needs_reverse {
        build_reverse_index(store)
    } else {
        ReverseIndex::default()
    };

    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        if let Some(v) = mem_filter
            && entity.mem != v
        {
            continue;
        }
        let Some(mem_schema) = mem_schemas.get(entity.mem.as_str()) else {
            continue;
        };
        let Some(td) = mem_schema.types.get(entity.entity_type.as_str()) else {
            continue;
        };

        // Section-format sweep (plan 08) — standing violations of a
        // declared markdown shape, every severity (block-tier
        // pre-existing violations are health findings too; the next
        // write of the section is the sanctioned repair point).
        for def in &td.sections {
            if def.compiled_content.is_none() {
                continue;
            }
            let Some(body) = entity.sections.get(def.key.as_str()) else {
                continue;
            };
            let violations = crate::section_format::check_section_format(def, body);
            if !violations.is_empty() {
                by_entity
                    .entry(entity.id.0.clone())
                    .or_default()
                    .1
                    .extend(violations);
            }
        }

        // Reachability obligations — health-sweep only by design (no
        // single write completes a transitive absence, so the write
        // path never evaluates these). The finding echoes the whole
        // declaration.
        for ob in &td.must_reach {
            if !reaches_terminal(store, &reverse, &entity.id, ob) {
                by_entity.entry(entity.id.0.clone()).or_default().0.push(
                    UnsatisfiedConstraint::MustReach {
                        relationships: ob.relationships.clone(),
                        direction: ob.direction,
                        terminal_types: ob.terminal_types.clone(),
                        max_depth: ob.max_depth,
                        severity: ob.severity,
                    },
                );
            }
        }

        if td.constraints.is_empty() {
            continue;
        }

        // Pass 1 — per-entity forms.
        let violations = unsatisfied_constraints(store, entity, td, None, checks);
        if !violations.is_empty() {
            by_entity
                .entry(entity.id.0.clone())
                .or_default()
                .0
                .extend(violations);
        }

        // Pass 2 — this entity as a taint source: it holds a declared
        // terminal value, so sweep its dependents. The taint walks
        // the declared relation set's union subgraph — a single
        // `rel_type` is a one-element set.
        for c in &td.constraints {
            let ConstraintDef::StatusPropagation {
                field,
                value,
                rel_type,
                rel_types,
                direction,
                severity,
            } = c
            else {
                continue;
            };
            let terminal = entity
                .metadata
                .get(field.as_str())
                .is_some_and(|v| v.to_frontmatter_string() == *value);
            if !terminal {
                continue;
            }
            let set = c
                .propagation_rel_types()
                .expect("StatusPropagation always yields a set");
            for tainted in reach_transitively(store, &entity.id, &set, *direction) {
                if let Some(v) = mem_filter
                    && tainted.mem() != v
                {
                    continue;
                }
                by_entity.entry(tainted.0.clone()).or_default().0.push(
                    UnsatisfiedConstraint::StatusPropagation {
                        field: field.clone(),
                        value: value.clone(),
                        rel_type: rel_type.clone(),
                        rel_types: rel_types.clone(),
                        tainted_by: entity.id.to_string(),
                        severity: *severity,
                    },
                );
            }
        }
    }

    let mut out: Vec<ConstraintFindingReport> = by_entity
        .into_iter()
        .filter_map(|(id, (violations, format_violations))| {
            let id = crate::entity::EntityId(id);
            let entity = store.get(&id)?;
            Some(ConstraintFindingReport {
                id,
                title: entity.title.clone(),
                entity_type: entity.entity_type.clone(),
                mem: entity.mem.clone(),
                violations,
                format_violations,
            })
        })
        .collect();
    out.sort_by(|a, b| a.mem.cmp(&b.mem).then_with(|| a.id.0.cmp(&b.id.0)));
    out
}

/// Transitive reachability along one rel-type from `start`, excluding
/// `start` itself. `Incoming` walks against edge direction (the
/// entities whose `rel_type` edges point at the frontier — "what
/// stands on this"); `Outgoing` follows the frontier's own edges.
/// Stubs are traversed (an edge through a stub still transmits the
/// taint) but stubs themselves are not returned.
fn reach_transitively(
    store: &Store,
    start: &crate::entity::EntityId,
    rel_types: &[String],
    direction: memstead_schema::PropagationDirection,
) -> Vec<crate::entity::EntityId> {
    use memstead_schema::PropagationDirection;
    let mut seen: std::collections::HashSet<crate::entity::EntityId> =
        std::iter::once(start.clone()).collect();
    let mut frontier = vec![start.clone()];
    let mut reached = Vec::new();
    while let Some(current) = frontier.pop() {
        let next: Vec<crate::entity::EntityId> = match direction {
            PropagationDirection::Incoming => store
                .all_entities()
                .filter(|e| {
                    e.relationships
                        .iter()
                        .any(|r| rel_types.iter().any(|n| n == &r.rel_type) && r.target == current)
                })
                .map(|e| e.id.clone())
                .collect(),
            PropagationDirection::Outgoing => store
                .get(&current)
                .map(|e| {
                    e.relationships
                        .iter()
                        .filter(|r| rel_types.iter().any(|n| n == &r.rel_type))
                        .map(|r| r.target.clone())
                        .collect()
                })
                .unwrap_or_default(),
        };
        for id in next {
            if seen.insert(id.clone()) {
                if store.get(&id).is_some_and(|e| !e.stub) {
                    reached.push(id.clone());
                }
                frontier.push(id);
            }
        }
    }
    reached
}

/// Reverse adjacency for `must_reach` incoming walks: target id →
/// `(rel_type, source id)` pairs. Built once per sweep so the
/// incoming direction stays O(edges) instead of re-scanning the store
/// per frontier node.
type ReverseIndex =
    std::collections::HashMap<crate::entity::EntityId, Vec<(String, crate::entity::EntityId)>>;

fn build_reverse_index(store: &Store) -> ReverseIndex {
    let mut idx = ReverseIndex::default();
    for entity in store.all_entities() {
        for rel in &entity.relationships {
            idx.entry(rel.target.clone())
                .or_default()
                .push((rel.rel_type.clone(), entity.id.clone()));
        }
    }
    idx
}

/// Whether `start` reaches at least one non-stub entity of a terminal
/// type along the obligation's relation set, direction, and depth
/// bound. Breadth-first with visited-set discipline (cycles along the
/// walked set terminate); stubs terminate no obligation — they carry
/// no outgoing edges and never count as reached terminals. Cross-mem
/// edges are followed like any edge, matching the propagation walk
/// and the cycle check (the engine's established traversal posture);
/// the start entity itself never satisfies its own obligation.
fn reaches_terminal(
    store: &Store,
    reverse: &ReverseIndex,
    start: &crate::entity::EntityId,
    ob: &memstead_schema::MustReach,
) -> bool {
    use memstead_schema::ReachDirection;
    let mut seen: std::collections::HashSet<crate::entity::EntityId> =
        std::iter::once(start.clone()).collect();
    let mut frontier = vec![start.clone()];
    let mut depth: u32 = 0;
    while !frontier.is_empty() {
        if let Some(max) = ob.max_depth
            && depth >= max
        {
            return false;
        }
        depth += 1;
        let mut next_frontier = Vec::new();
        for current in frontier {
            let next: Vec<crate::entity::EntityId> = match ob.direction {
                ReachDirection::Out => store
                    .get(&current)
                    .map(|e| {
                        e.relationships
                            .iter()
                            .filter(|r| ob.relationships.iter().any(|n| n == &r.rel_type))
                            .map(|r| r.target.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
                ReachDirection::In => reverse
                    .get(&current)
                    .map(|sources| {
                        sources
                            .iter()
                            .filter(|(rel, _)| ob.relationships.iter().any(|n| n == rel))
                            .map(|(_, src)| src.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            for id in next {
                if seen.insert(id.clone()) {
                    if store.get(&id).is_some_and(|e| {
                        !e.stub && ob.terminal_types.iter().any(|t| t == &e.entity_type)
                    }) {
                        return true;
                    }
                    next_frontier.push(id);
                }
            }
        }
        frontier = next_frontier;
    }
    false
}

/// One entity's above-`none` signals, surfaced from the include-gated
/// `signals` health axis. Mirrors [`ConstraintFindingReport`]'s
/// envelope shape; the `signals` entries carry value, level, and
/// contributors (the evidence ships with the number, always).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SignalReport {
    pub id: crate::entity::EntityId,
    pub title: String,
    pub entity_type: String,
    pub mem: String,
    /// Only signals whose level is not `none`, in declaration order.
    pub signals: Vec<super::signals::ComputedSignal>,
}

impl SignalReport {
    /// Whether any entry is `warn`-level — the `--strict`
    /// participation test (a `notice` never participates; that is the
    /// whole difference between the two levels).
    pub fn has_warn(&self) -> bool {
        self.signals
            .iter()
            .any(|s| s.level == Some(memstead_schema::SignalLevel::Warn))
    }
}

/// Collect every non-stub entity carrying at least one declared
/// signal above `none`. Deterministic — sorted by `(mem, id)`;
/// signals in declaration order, contributors sorted.
pub fn collect_signal_reports(
    store: &Store,
    mem_filter: Option<&str>,
    mem_schemas: &HashMap<String, Arc<memstead_schema::Schema>>,
) -> Vec<SignalReport> {
    let mut out = Vec::new();
    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        if let Some(v) = mem_filter
            && entity.mem != v
        {
            continue;
        }
        let Some(mem_schema) = mem_schemas.get(entity.mem.as_str()) else {
            continue;
        };
        let Some(td) = mem_schema.types.get(entity.entity_type.as_str()) else {
            continue;
        };
        if td.signals.is_empty() {
            continue;
        }
        let above: Vec<super::signals::ComputedSignal> =
            super::signals::compute_signals(store, td, &entity.id)
                .into_iter()
                .filter(|s| s.level.is_some())
                .collect();
        if above.is_empty() {
            continue;
        }
        out.push(SignalReport {
            id: entity.id.clone(),
            title: entity.title.clone(),
            entity_type: entity.entity_type.clone(),
            mem: entity.mem.clone(),
            signals: above,
        });
    }
    out.sort_by(|a, b| a.mem.cmp(&b.mem).then_with(|| a.id.0.cmp(&b.id.0)));
    out
}

/// A defective section-format declaration a loaded schema carries
/// (recorded by the lenient boot path; install would have refused).
/// Surfaced under the health `constraints` include so a sealed schema
/// with a bad declaration is visible without bricking boot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SchemaFormatDefect {
    pub schema: String,
    pub type_name: String,
    pub section: String,
    pub problems: Vec<String>,
}

/// Collect the defective section-format declarations across the
/// mounted mems' pinned schemas, deduplicated per schema ref,
/// deterministic order.
pub fn collect_schema_format_defects(
    mem_schemas: &HashMap<String, Arc<memstead_schema::Schema>>,
) -> Vec<SchemaFormatDefect> {
    let mut seen: std::collections::BTreeSet<String> = Default::default();
    let mut out = Vec::new();
    let mut schemas: Vec<&Arc<memstead_schema::Schema>> = mem_schemas.values().collect();
    schemas.sort_by_key(|s| (s.manifest.name.clone(), s.version.clone()));
    for schema in schemas {
        let schema_ref = format!("{}@{}", schema.manifest.name, schema.version);
        if !seen.insert(schema_ref.clone()) {
            continue;
        }
        for td in schema.types.values() {
            for section in &td.sections {
                if !section.format_problems.is_empty() {
                    out.push(SchemaFormatDefect {
                        schema: schema_ref.clone(),
                        type_name: td.name.clone(),
                        section: section.key.clone(),
                        problems: section.format_problems.clone(),
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| {
        (&a.schema, &a.type_name, &a.section).cmp(&(&b.schema, &b.type_name, &b.section))
    });
    out
}

/// One entity's unsatisfied `required_outgoing` blocks, surfaced from
/// the health-time scan. `missing` reuses the per-write warning's wire
/// block type — one struct, one serialized shape (`{ relationships,
/// cardinality }`) on both surfaces — and adds the `mem` name (the
/// warning's `entity_id` already encodes it via the mem prefix, but
/// health is multi-mem by default and an explicit field is cheaper for
/// downstream filters).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MissingRequiredOutgoingReport {
    pub id: crate::entity::EntityId,
    pub title: String,
    pub entity_type: String,
    pub mem: String,
    pub missing: Vec<super::MissingRequiredOutgoingBlock>,
}

/// Render the workspace-config projection the health surface serves —
/// per-writable-mem detail (`origin`, storage/durability, `vcs`
/// `gitdir`/`worktree`/`head`, title/subject, `write_guidance`,
/// `extra`) plus the `mutations` and `plugin` policy values. One
/// implementation, every surface: the MCP composer reaches it through
/// `include_config: true` OR the `config` include key; the CLI through
/// `--include config`. `mutations` / `plugin` are passed prebuilt so a
/// server that owns its own copies inserts them verbatim; callers
/// without server state derive them from `Engine::settings()` (see
/// [`config_projection_from_settings`]). Returns the three top-level
/// entries (`mems`, `mutations`, `plugin`) for the caller to merge —
/// callers gate on their own opt-in flag and must render at most once.
pub fn config_projection(
    engine: &crate::Engine,
    writable_mems: &[String],
    mutations: serde_json::Value,
    plugin: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    // Per-mem storage backend → durability marker, derived from the
    // mount's `MountStorage` kind. Lives alongside `vcs` so an agent
    // reading per-mem config learns whether a `write_id` this mem
    // returns is durable-on-disk or volatile-in-RAM.
    let backend_by_mem: std::collections::HashMap<&str, (&'static str, bool)> = engine
        .mounts()
        .iter()
        .map(|m| {
            (
                m.mem.as_str(),
                (m.storage.backend_id(), m.storage.is_durable()),
            )
        })
        .collect();
    let mems_detail: Vec<serde_json::Value> = writable_mems
        .iter()
        .map(|name| {
            let origin = engine
                .mem_router()
                .origin_for_mem(name)
                .map(|o| o.kind())
                .unwrap_or("explicit");
            let mut entry = serde_json::Map::new();
            entry.insert("name".into(), serde_json::json!(name));
            entry.insert("origin".into(), serde_json::json!(origin));
            if let Some((storage, durable)) = backend_by_mem.get(name.as_str()).copied() {
                entry.insert("storage".into(), serde_json::json!(storage));
                entry.insert("durable".into(), serde_json::json!(durable));
            }
            let mut vcs_obj = serde_json::Map::new();
            if let Ok(gitdir) = engine.gitdir_for(name) {
                vcs_obj.insert("gitdir".into(), serde_json::json!(gitdir));
            }
            if let Ok(worktree) = engine.worktree_for(name) {
                vcs_obj.insert("worktree".into(), serde_json::json!(worktree));
            }
            if let Some(sha) = engine.mem_head_sha(name).ok().flatten() {
                vcs_obj.insert("head".into(), serde_json::json!(sha));
            }
            if !vcs_obj.is_empty() {
                entry.insert("vcs".into(), serde_json::Value::Object(vcs_obj));
            }
            if let Some(cfg) = engine.mem_config_for(name) {
                // Display title + subject block, when set — the
                // config projection prefers the title wherever a
                // mem is printed; the name stays the identity.
                if let Some(title) = &cfg.title {
                    entry.insert("title".into(), serde_json::json!(title));
                }
                if let Some(subject) = &cfg.subject {
                    entry.insert("subject".into(), serde_json::json!(subject));
                }
                let guidance = serde_json::Map::from_iter(
                    cfg.write_guidance
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone())),
                );
                entry.insert("write_guidance".into(), serde_json::Value::Object(guidance));
                let extra = serde_json::Map::from_iter(
                    cfg.extra.iter().map(|(k, v)| (k.clone(), v.clone())),
                );
                entry.insert("extra".into(), serde_json::Value::Object(extra));
            }
            serde_json::Value::Object(entry)
        })
        .collect();

    let mut out = serde_json::Map::new();
    out.insert("mems".into(), serde_json::json!(mems_detail));
    out.insert("mutations".into(), mutations);
    out.insert("plugin".into(), plugin);
    out
}

/// The `(mutations, plugin)` pair for [`config_projection`], derived
/// from the engine's own [`crate::workspace::WorkspaceSettings`] — for
/// callers (the CLI) that carry no server-owned config copies. Produces
/// the same bytes the MCP server passes when both were loaded from the
/// same `workspace.toml`.
pub fn config_projection_from_settings(
    settings: &crate::workspace::WorkspaceSettings,
) -> (serde_json::Value, serde_json::Value) {
    let mutations = serde_json::json!({ "require_notes": settings.mutations.require_notes });
    let plugin_map: serde_json::Map<String, serde_json::Value> = settings
        .plugin
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    (mutations, serde_json::Value::Object(plugin_map))
}

/// Detect the section-fork condition for one declared section: the
/// parsed content under `key` is empty, the schema's declared heading
/// for the key does not derive back to it
/// (`derive_section_key(heading) != key`), and the file carries that
/// declared heading — so the content is present in the file but
/// unreachable under the key: absorbed into the catch-all when the
/// type declares one, dropped from the parsed sections otherwise.
///
/// Returns the distinct `SECTION_HEADING_MISMATCH` issue naming both
/// the found heading and what a deriving heading would look like. The
/// caller must NOT also report the section as missing — collapsing the
/// two conditions into the missing-section report is exactly the
/// misdirection this finding exists to prevent (the operator goes
/// hunting for absent content that is in fact present).
pub(crate) fn section_heading_mismatch_issue(
    entity: &crate::entity::Entity,
    schema: &TypeDefinition,
    key: &str,
) -> Option<HealthIssue> {
    let def = schema.section(key)?;
    let derived = memstead_schema::derive_section_key(&def.heading);
    if derived == key {
        return None;
    }
    if !entity
        .raw_section_headings
        .iter()
        .any(|h| h == &def.heading)
    {
        return None;
    }
    let landing = match schema.catch_all_section() {
        Some(c) => format!(
            "the content was absorbed into catch-all section '{}'",
            c.key
        ),
        None => "the content is unreachable under any declared key".to_string(),
    };
    Some(HealthIssue {
        field: key.to_string(),
        code: super::HealthIssueCode::SectionHeadingMismatch,
        message: format!(
            "SECTION_HEADING_MISMATCH: section '{key}' is not missing — its content sits \
             under heading '{found}', which derives to '{derived}', not '{key}'; {landing}. \
             The schema's declared heading cannot round-trip to its key (expected a heading \
             that derives to '{key}'); fix the schema's heading/key pair — new installs of \
             such a schema are refused",
            found = def.heading,
        ),
    })
}

/// Get a single entity's health report.
pub fn entity_health(entity: &crate::entity::Entity, schema: &TypeDefinition) -> HealthReport {
    let mut issues = Vec::new();

    for field in &schema.health_required_fields {
        if schema.section(field).is_some() {
            let content = entity.sections.get(field.as_str());
            if content.is_none_or(|c| c.trim().is_empty()) {
                if let Some(issue) = section_heading_mismatch_issue(entity, schema, field) {
                    issues.push(issue);
                } else {
                    issues.push(HealthIssue {
                        field: field.clone(),
                        code: super::HealthIssueCode::Missing,
                        message: format!("required section '{field}' is empty"),
                    });
                }
            }
        } else {
            let value = entity.metadata.get(field.as_str());
            if value.is_none() {
                issues.push(HealthIssue {
                    field: field.clone(),
                    code: super::HealthIssueCode::Missing,
                    message: format!("required field '{field}' is missing"),
                });
            }
        }
    }

    // The mismatch condition is drift worth surfacing on every declared
    // section, not only the health-required ones — an optional section
    // whose content forked away is just as invisible to readers.
    for s in schema.sections.iter().filter(|s| !s.catch_all) {
        if schema.health_required_fields.contains(&s.key) {
            continue; // already handled above
        }
        let content = entity.sections.get(s.key.as_str());
        if content.is_none_or(|c| c.trim().is_empty())
            && let Some(issue) = section_heading_mismatch_issue(entity, schema, &s.key)
        {
            issues.push(issue);
        }
    }

    let total = schema.health_required_fields.len();
    let score = if total > 0 {
        (total.saturating_sub(issues.len()) as f32) / (total as f32)
    } else {
        1.0
    };

    HealthReport {
        id: entity.id.clone(),
        title: entity.title.clone(),
        score,
        issues,
    }
}

// ---------------------------------------------------------------------------
// Date helpers
// ---------------------------------------------------------------------------

/// Get current days since Unix epoch.
///
/// `SystemTime::now()` is unimplemented on `wasm32-unknown-unknown` —
/// it traps with `RuntimeError: unreachable` and poisons the wasm
/// instance (cold-start F11) — so the wasm build reads the JS-backed
/// clock instead. Same value, same summary shape on every target.
/// Today as whole days since the Unix epoch, the clock every day-threshold
/// reading in health uses. `MEMSTEAD_TODAY=YYYY-MM-DD` pins it (the
/// injection a fixture needs to age entities without waiting), honoured
/// by every binary alike so the CLI and the MCP server read the same day.
pub fn days_since_epoch() -> u64 {
    if let Some(pinned) = std::env::var("MEMSTEAD_TODAY")
        .ok()
        .and_then(|s| crate::engine::due::pinned_days_since_epoch(&s))
    {
        return pinned;
    }
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0) as u64 / 86400
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / 86400
    }
}

/// Parse an ISO 8601 date string to days since epoch.
/// Supports `YYYY-MM-DD` and `YYYY-MM-DDTHH:MM:SSZ`.
pub fn parse_iso_to_days(date: &str) -> Option<u64> {
    let date_part = date.split('T').next()?;
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: u64 = parts[0].parse().ok()?;
    let month: u64 = parts[1].parse().ok()?;
    let day: u64 = parts[2].parse().ok()?;
    Some(ymd_to_days(year, month, day))
}

/// Convert (year, month, day) to days since Unix epoch.
/// Inverse of the algorithm in generator.rs.
fn ymd_to_days(year: u64, month: u64, day: u64) -> u64 {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let y = if month <= 2 { year - 1 } else { year };
    let m = if month <= 2 { month + 9 } else { month - 3 };
    let era = y / 400;
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe;
    days - 719468
}

#[cfg(test)]
mod tests;
