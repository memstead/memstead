//! Shared health composer used by the `memstead_health` MCP tool and any
//! non-MCP caller (CLI, a future HTTP surface).
//!
//! Lifted from `memstead-mcp/src/server.rs::memstead_health_unified` so the
//! health read-envelope is produced by one transport-neutral builder with no
//! rmcp type in the path — the MCP wrapper handles drift collection, the
//! schema anchor, the `mem_changed` notice channel, and `CallToolResult`
//! wrapping; none of that lives here.
//!
//! The composer returns the complete health payload as a `serde_json::Value`
//! (warnings embedded, every `include` detail section applied). Surface state
//! the engine does not own — the `[mutations]` posture and the opaque
//! `[plugin.*]` map — is passed in via [`HealthConfig`] as prebuilt JSON so
//! this crate stays free of the MCP server's config types and the wire bytes
//! stay identical to the pre-lift handler.

use std::collections::HashMap;

/// Composer input — packed from the MCP `HealthParams` (or a CLI `Args`) at
/// the call site. Mirrors the field set the pre-lift handler read off
/// `HealthParams`.
#[derive(Debug)]
pub struct HealthArgs<'a> {
    pub mem: Option<&'a str>,
    pub include: &'a [String],
    pub limit: Option<usize>,
    pub target_schema: Option<&'a str>,
    pub include_config: bool,
}

/// Surface-owned config the engine does not carry — supplied prebuilt so the
/// composer inserts the bytes verbatim. `mutations` is `{"require_notes": …}`;
/// `plugin` is the opaque `[plugin.*]` pass-through object. Only consulted
/// when `args.include_config` is set.
#[derive(Debug, Clone)]
pub struct HealthConfig {
    pub mutations: serde_json::Value,
    pub plugin: serde_json::Value,
}

/// Typed input failures the composer surfaces. The MCP wrapper maps each
/// variant to its existing envelope (`UNKNOWN_MEM`, `INVALID_INPUT`) and
/// the engine fault to its typed translator, so the wire `code` stays put.
#[derive(Debug, thiserror::Error)]
pub enum ComposeHealthError {
    /// `args.mem` names a mem that isn't writable in this workspace. The
    /// composer surfaces the sorted writable roster so the wrapper can echo
    /// it in the `UNKNOWN_MEM` envelope.
    #[error("unknown mem: \"{name}\"")]
    UnknownMem {
        name: String,
        writable_mems: Vec<String>,
    },
    /// `args.target_schema` did not parse as a `name@x.y.z` ref. `reason` is
    /// the parser's message, surfaced verbatim in the `INVALID_INPUT`
    /// envelope's `details.reason`.
    #[error("invalid target_schema {raw:?}: {reason}")]
    InvalidTargetSchema { raw: String, reason: String },
    /// A backend fault from the conformance / consistency scan. The wrapper
    /// routes it through the typed `EngineError` translator unchanged.
    #[error(transparent)]
    Engine(#[from] memstead_base::EngineError),
}

/// Build the complete health payload. `drift_warnings` are the reload warnings
/// the wrapper collected before calling in; the composer extends them with the
/// health report's own warnings, the limit-clamp notice, and unknown-include
/// notices, then embeds the lot under `warnings`.
pub fn compose_health(
    engine: &mut memstead_base::Engine,
    args: &HealthArgs,
    drift_warnings: Vec<memstead_base::WarningHint>,
    config: &HealthConfig,
) -> Result<serde_json::Value, ComposeHealthError> {
    let health = engine.health();
    let stats = engine.stats();
    let include = args.include;
    const HEALTH_LIMIT_MAX: usize = 100;
    let requested_limit = args.limit.unwrap_or(10);
    let limit = requested_limit.min(HEALTH_LIMIT_MAX);

    let mut warnings: Vec<memstead_base::WarningHint> = drift_warnings;
    warnings.extend(health.warnings.clone());
    if requested_limit > HEALTH_LIMIT_MAX {
        warnings.push(memstead_base::WarningHint::LimitClamped {
            requested: requested_limit,
            actual: HEALTH_LIMIT_MAX,
        });
    }

    for key in include {
        if !memstead_base::ops::health::HEALTH_INCLUDE_KEYS.contains(&key.as_str()) {
            warnings.push(memstead_base::WarningHint::UnknownIncludeKey {
                key: key.clone(),
                allowed: memstead_base::ops::health::HEALTH_INCLUDE_KEYS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            });
        }
    }

    // Mem filter validation — only writable mems accepted.
    let mem_filter: Option<String> = match args.mem {
        Some(v) if engine.mem_router().is_writable(v) => Some(v.to_string()),
        Some(v) => {
            let mut names: Vec<String> = engine
                .mem_router()
                .writable_mems()
                .iter()
                .cloned()
                .collect();
            names.sort();
            return Err(ComposeHealthError::UnknownMem {
                name: v.to_string(),
                writable_mems: names,
            });
        }
        None => None,
    };
    let vf = mem_filter.as_deref();

    // Symmetric with the data filter below: mem-attributable warnings
    // (SUSPICIOUS_NESTED_PREFIX, DUPLICATE_SECTION_HEADING, etc.) drop out when
    // their source mem isn't the scoped one. Workspace- and request-scoped
    // warnings (OUTER_REPO_…, UNKNOWN_INCLUDE_KEY, LIMIT_CLAMPED) report `None`
    // from `source_mem()` and stay visible — agents should see them
    // regardless of which mem they're scoping to.
    if let Some(v) = vf {
        warnings.retain(|w| w.source_mem().is_none_or(|wv| wv == v));
    }

    let in_mem = |e: &memstead_base::Entity| -> bool {
        match vf {
            Some(v) => e.mem == v,
            None => true,
        }
    };
    let real_count = engine
        .store()
        .all_entities()
        .filter(|e| !e.stub && in_mem(e))
        .count();
    let stub_count = engine
        .store()
        .all_entities()
        .filter(|e| e.stub && in_mem(e))
        .count();
    let total_count = real_count + stub_count;

    let orphan_ids: Vec<memstead_base::EntityId> = engine
        .orphans()
        .into_iter()
        .filter(|id| match vf {
            Some(v) => engine
                .store()
                .get(id)
                .map(|e| e.mem == v)
                .unwrap_or(false),
            None => true,
        })
        .collect();
    let stub_pairs: Vec<(memstead_base::EntityId, Vec<memstead_base::EntityId>)> = engine
        .stubs()
        .into_iter()
        .filter(|(id, _)| match vf {
            Some(v) => engine
                .store()
                .get(id)
                .map(|e| e.mem == v)
                .unwrap_or(false),
            None => true,
        })
        .collect();

    // Under a `mem` filter, scope the community count to clusters with ≥1
    // member in that mem (filtering the global partition, not re-running
    // detection) so it can't contradict the scoped `total_entities` — e.g. an
    // empty mem reports 0 entities and 0 communities. Mirrors
    // `memstead_overview` via the shared helper.
    let community_count = match vf {
        Some(v) => {
            memstead_base::graph::community::clusters_in_mem(engine.store(), engine.communities(), v)
                .len()
        }
        None => engine.communities().count,
    };

    // Edge counts: under a mem filter, count only source-in-mem edges
    // (asymmetric — matches the legacy contract).
    let (edge_count, edge_types) = {
        if let Some(v) = vf {
            let mut counts: HashMap<String, usize> = HashMap::new();
            let mut total: usize = 0;
            for id in engine.store().all_ids() {
                let source_mem = engine.store().get(id).map(|e| e.mem.clone());
                if let Some(source) = source_mem.as_deref()
                    && source != v
                {
                    continue;
                }
                for edge in engine.store().outgoing(id) {
                    *counts.entry(edge.rel_type.clone()).or_insert(0) += 1;
                    total += 1;
                }
            }
            let mut pairs: Vec<_> = counts.into_iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(&a.1));
            let arr: Vec<serde_json::Value> = pairs
                .into_iter()
                .map(|(t, c)| serde_json::json!({"type": t, "count": c}))
                .collect();
            (total, arr)
        } else {
            let mut pairs: Vec<_> = stats.edge_types.iter().collect();
            pairs.sort_by(|a, b| b.1.cmp(a.1));
            let arr: Vec<serde_json::Value> = pairs
                .into_iter()
                .map(|(t, c)| serde_json::json!({"type": t, "count": c}))
                .collect();
            (stats.edge_count, arr)
        }
    };

    let type_distribution: Vec<serde_json::Value> = {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for e in engine.store().all_entities().filter(|e| !e.stub && in_mem(e)) {
            *counts.entry(&e.entity_type).or_default() += 1;
        }
        let mut pairs: Vec<_> = counts.into_iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(&a.1));
        pairs
            .into_iter()
            .map(|(s, c)| serde_json::json!({"type": s, "count": c}))
            .collect()
    };

    let writable_mems: Vec<String> = {
        let mut names: Vec<String> = engine
            .mem_router()
            .writable_mems()
            .iter()
            .cloned()
            .collect();
        names.sort();
        names
    };
    // The stable default an omitted-`mem` mutation lands in — the first
    // writable mount in declaration order, not `writable_mems[0]` of this
    // alphabetically-sorted roster. Surfaced so an omitted-`mem` write is
    // predictable.
    let default_writable_mem: Option<String> =
        engine.default_writable_mem().map(|s| s.to_string());
    let read_mems: Vec<String> = {
        let writable_set: std::collections::HashSet<&String> =
            engine.mem_router().writable_mems().iter().collect();
        let mut names: Vec<String> = engine
            .mem_router()
            .visible_mems()
            .iter()
            .filter(|n| !writable_set.contains(*n))
            .cloned()
            .collect();
        names.sort();
        names
    };

    // Per-mem schema pins. Source from `engine.mount(name).schema` which
    // carries the pinned `SchemaRef`; render via `as_display()` to get the
    // same `name@version` form pro emits.
    //
    // Every *visible* mem appears — writable and read-only alike — each
    // carrying an explicit `writable` attribute. A read-only mount's pinned
    // schema is real; surfacing it here (rather than filtering to writable
    // mems) is what keeps health from reporting "no schema" while the
    // discovery manifest names one. Writable mems render first (sorted),
    // then read-only ones (sorted), so a normal writable workspace — which
    // has no read mems — keeps its existing entry order, gaining only the
    // `writable: true` attribute.
    let mem_schemas: Vec<serde_json::Value> = {
        let mut entries: Vec<serde_json::Value> = Vec::new();
        let writable_set: std::collections::HashSet<&String> = writable_mems.iter().collect();
        for name in writable_mems.iter().chain(read_mems.iter()) {
            if let Some(v) = vf
                && name != v
            {
                continue;
            }
            if let Some(m) = engine.mount(name) {
                // The mem's *settled* pin — `Mount.schema` (now an optional
                // assertion). During a dual-pin migration this stays the
                // settled pin; the in-flight target is the separate
                // `migration_target` surface below.
                let schema_ref = m
                    .schema
                    .as_ref()
                    .map(|s| s.as_display())
                    .unwrap_or_default();
                let mut entry = serde_json::json!({
                    "mem": name,
                    "schema": schema_ref,
                    "writable": writable_set.contains(name),
                });
                // Dual-pin confirmation surface: present only while a
                // migration is in flight, so settled mems' entries stay
                // byte-identical to before.
                if let Some(target) = &m.migration_target {
                    entry["migration_target"] = serde_json::json!(target.as_display());
                }
                entries.push(entry);
            }
        }
        entries
    };

    // #49: segment the orphan / community headlines by the owning mem's
    // schema. A blended total mixes schemas with opposite norms — ingest
    // mems, where each finding is an isolated entity (orphan by design),
    // versus code/spec mems, where an orphan is real debt — so a bare
    // "54 orphans" reads as 54 units of debt when most are by-design
    // isolates. The raw `total_orphans` / `total_communities` are retained
    // in `summary`, and a `mem`-scoped call still exposes per-mem counts
    // (the refusal AC); these maps only attribute the totals by schema.
    // `orphan_ids` is already mem-scoped above; scope the community
    // attribution to the same mem set.
    let orphans_by_schema = engine.orphans_by_schema(&orphan_ids);
    let scope_mems: Vec<String> = match vf {
        Some(v) => vec![v.to_string()],
        None => writable_mems
            .iter()
            .chain(read_mems.iter())
            .cloned()
            .collect(),
    };
    let communities_by_schema = engine.communities_by_schema(&scope_mems);

    let mut result = serde_json::json!({
        "mem": mem_filter,
        "summary": {
            "total_entities": real_count,
            "total_orphans": orphan_ids.len(),
            "total_stubs": stub_pairs.len(),
            "total_stale": health.stale_entities.iter().filter(|e| match vf {
                Some(v) => engine.store().get(&e.id).map(|ent| ent.mem == v).unwrap_or(false),
                None => true,
            }).count(),
            "total_missing_fields": health.missing_fields.iter().filter(|h| match vf {
                Some(v) => engine.store().get(&h.id).map(|ent| ent.mem == v).unwrap_or(false),
                None => true,
            }).count(),
            "total_communities": community_count,
            "orphans_by_schema": orphans_by_schema,
            "communities_by_schema": communities_by_schema,
        },
        "total_nodes": total_count,
        "real_nodes": real_count,
        "stub_nodes": stub_count,
        "total_edges": edge_count,
        "edge_types": edge_types,
        "type_distribution": type_distribution,
        "writable_mems": writable_mems,
        "default_writable_mem": default_writable_mem,
        "read_mems": read_mems,
        "mem_schemas": mem_schemas,
    });
    let obj = result.as_object_mut().unwrap();
    if !warnings.is_empty() {
        obj.insert("warnings".into(), serde_json::json!(warnings));
    }

    if include.iter().any(|s| s == "orphans") {
        let orphans_list: Vec<serde_json::Value> = orphan_ids
            .into_iter()
            .map(|id| {
                let title = engine
                    .get_entity(&id)
                    .map(|e| e.title.clone())
                    .unwrap_or_default();
                serde_json::json!({"id": id.to_string(), "title": title})
            })
            .collect();
        obj.insert("orphans".into(), serde_json::json!(orphans_list));
    }
    if include.iter().any(|s| s == "stubs") {
        let stubs_list: Vec<serde_json::Value> = stub_pairs
            .into_iter()
            .map(|(id, refs)| {
                serde_json::json!({
                    "id": id.to_string(),
                    "referenced_by": refs.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                })
            })
            .collect();
        obj.insert("stubs".into(), serde_json::json!(stubs_list));
    }
    if include.iter().any(|s| s == "most_connected") {
        use memstead_base::graph::query::{cmp_by_dependency, connectivity_for, Connectivity};
        // `typed_*` is the dependency degree (excludes auto-emitted mention
        // edges); the list is ranked by it so a co-mention hub doesn't
        // outrank a real dependency hub. `total`/`incoming`/`outgoing` keep
        // the mentions and stay available — mention degree = total - typed.
        let to_json = |c: Connectivity| {
            let title = engine
                .get_entity(&c.id)
                .map(|e| e.title.clone())
                .unwrap_or_default();
            serde_json::json!({
                "id": c.id.to_string(),
                "title": title,
                "total": c.total,
                "incoming": c.incoming,
                "outgoing": c.outgoing,
                "typed_total": c.typed_total,
                "typed_incoming": c.typed_incoming,
                "typed_outgoing": c.typed_outgoing,
            })
        };
        let connected: Vec<serde_json::Value> = if let Some(v) = vf {
            // Source-in-mem scoping, to match this response's `edge_types`
            // / `total_edges`. The node is in-mem, so all of its outgoing
            // edges are source-in-mem and counted; an incoming edge counts
            // only when its source is also in-mem, so a cross-mem edge
            // the aggregate excluded does not inflate the node's degree here.
            let mut entries: Vec<Connectivity> = engine
                .store()
                .all_entities()
                .filter(|e| !e.stub && e.mem == v)
                .map(|e| {
                    connectivity_for(engine.store(), &e.id, |in_edge| in_edge.from.mem() == v)
                })
                .collect();
            entries.sort_by(cmp_by_dependency);
            entries.truncate(limit);
            entries.into_iter().map(to_json).collect()
        } else {
            engine.most_connected(limit).into_iter().map(to_json).collect()
        };
        obj.insert("most_connected".into(), serde_json::json!(connected));
    }
    if include.iter().any(|s| s == "missing_fields") {
        let missing_fields: Vec<serde_json::Value> = health
            .missing_fields
            .iter()
            .filter(|h| match vf {
                Some(v) => engine
                    .store()
                    .get(&h.id)
                    .map(|e| e.mem == v)
                    .unwrap_or(false),
                None => true,
            })
            .map(|h| {
                let missing: Vec<&str> = h.issues.iter().map(|i| i.field.as_str()).collect();
                serde_json::json!({"id": h.id.to_string(), "title": h.title, "missing": missing})
            })
            .collect();
        obj.insert("missing_fields".into(), serde_json::json!(missing_fields));
    }
    if include.iter().any(|s| s == "stale") {
        let stale: Vec<serde_json::Value> = health
            .stale_entities
            .iter()
            .filter(|e| match vf {
                Some(v) => engine
                    .store()
                    .get(&e.id)
                    .map(|ent| ent.mem == v)
                    .unwrap_or(false),
                None => true,
            })
            .map(|e| {
                serde_json::json!({
                    "id": e.id.to_string(),
                    "title": e.title,
                    "days_since_modified": e.days_since_modified,
                })
            })
            .collect();
        obj.insert("stale".into(), serde_json::json!(stale));
    }
    if include.iter().any(|s| s == "dangling_links") {
        let dangling = memstead_base::ops::health::collect_dangling_links(engine.store(), vf);
        let arr: Vec<serde_json::Value> = dangling
            .into_iter()
            .map(|dl| serde_json::to_value(&dl).unwrap())
            .collect();
        obj.insert("dangling_links".into(), serde_json::json!(arr));
    }
    if include.iter().any(|s| s == "missing_required_outgoing") {
        let reports = engine.missing_required_outgoing(vf);
        let arr: Vec<serde_json::Value> = reports
            .into_iter()
            .map(|r| serde_json::to_value(&r).unwrap())
            .collect();
        obj.insert("missing_required_outgoing".into(), serde_json::json!(arr));
    }
    if include.iter().any(|s| s == "tags") {
        let (distribution, folded, untagged) =
            memstead_base::ops::health::collect_tag_distribution(engine.store(), vf, limit);
        obj.insert(
            "tag_distribution".into(),
            serde_json::to_value(&distribution).unwrap(),
        );
        obj.insert(
            "tag_distribution_folded".into(),
            serde_json::to_value(&folded).unwrap(),
        );
        obj.insert(
            "untagged_entities".into(),
            serde_json::to_value(&untagged).unwrap(),
        );
    }
    // Conformance axis (`conformance`), or both axes (`integrity`). Findings
    // ride one flat `findings` list in the pinned `{ id, axis, code, detail }`
    // shape; ids are mem-qualified so the flat list stays unambiguous when
    // unscoped. Mems scan in sorted order and each mem's findings are
    // deterministic, so the whole list is.
    let wants_conformance = include
        .iter()
        .any(|s| s == "conformance" || s == "integrity");
    if wants_conformance {
        let wants_consistency = include.iter().any(|s| s == "integrity");
        let target: Option<memstead_schema::SchemaRef> = match args.target_schema {
            None => None,
            Some(raw) => match raw.parse::<memstead_schema::SchemaRef>() {
                Ok(r) => Some(r),
                Err(reason) => {
                    return Err(ComposeHealthError::InvalidTargetSchema {
                        raw: raw.to_string(),
                        reason,
                    });
                }
            },
        };
        let scan_mems: Vec<String> = match vf {
            Some(v) => vec![v.to_string()],
            None => {
                let mut all = writable_mems.clone();
                all.sort();
                all
            }
        };
        let mut findings = Vec::new();
        for v in &scan_mems {
            findings.extend(engine.conformance_findings(v, target.as_ref())?);
            if wants_consistency {
                findings.extend(engine.consistency_findings(v)?);
            }
        }
        obj.insert("findings".into(), serde_json::to_value(&findings).unwrap());
    }

    // Workspace policy surface — opt-in `include_config: true`. Per-mem
    // `vcs: { gitdir?, worktree?, head? }` uses `gitdir_for` / `worktree_for`
    // / `mem_head_sha`. Each sub-field is conditionally present — folder
    // mounts have a worktree but no per-mem gitdir; freshly-created mems
    // have no head yet — and the `vcs` object emits whenever at least one of
    // them is available. `write_guidance` + `extra` come from
    // `mem_config_for`. `mutations` + `plugin` are passed in via
    // [`HealthConfig`] (server state the engine does not own).
    if args.include_config {
        // Per-mem storage backend → durability marker, derived from the
        // mount's `MountStorage` kind. Lives alongside `vcs` so an agent
        // reading per-mem config learns whether a `commit_sha` this mem
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
                    let guidance = serde_json::Map::from_iter(
                        cfg.write_guidance.iter().map(|(k, v)| (k.clone(), v.clone())),
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
        obj.insert("mems".into(), serde_json::json!(mems_detail));

        obj.insert("mutations".into(), config.mutations.clone());
        obj.insert("plugin".into(), config.plugin.clone());
    }

    Ok(result)
}

/// Render a composed health payload as a human-readable markdown report
/// for the MCP text channel. `structured_content` remains the source of
/// truth (this is never parsed back); the markdown exists so the text
/// channel is *chunkable* like `memstead_overview` instead of a wall of
/// JSON that overflows the response cap under several includes. The
/// size-driving include arrays each render as their own section so the
/// chunker can split a large report cleanly.
pub fn render_health_markdown(v: &serde_json::Value) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "# Graph health");
    if let Some(mem) = v.get("mem").and_then(|x| x.as_str()) {
        let _ = writeln!(s, "\nMem filter: `{mem}`");
    }

    if let Some(sum) = v.get("summary").and_then(|x| x.as_object()) {
        let _ = writeln!(s, "\n## Summary");
        for key in [
            "total_entities",
            "total_orphans",
            "total_stubs",
            "total_stale",
            "total_missing_fields",
            "total_communities",
        ] {
            if let Some(n) = sum.get(key).and_then(|x| x.as_u64()) {
                let _ = writeln!(s, "- {}: {n}", key.replace('_', " "));
            }
        }
        render_count_map(&mut s, sum.get("orphans_by_schema"), "Orphans by schema");
        render_count_map(
            &mut s,
            sum.get("communities_by_schema"),
            "Communities by schema",
        );
    }

    for key in ["total_nodes", "real_nodes", "stub_nodes", "total_edges"] {
        if let Some(n) = v.get(key).and_then(|x| x.as_u64()) {
            let _ = writeln!(s, "- {}: {n}", key.replace('_', " "));
        }
    }

    // Size-driving include arrays — one section each so chunking splits them.
    for (key, title) in [
        ("orphans", "Orphans"),
        ("stubs", "Stubs"),
        ("most_connected", "Most connected"),
        ("missing_fields", "Missing fields"),
        ("stale", "Stale"),
        ("dangling_links", "Dangling links"),
        ("missing_required_outgoing", "Missing required outgoing"),
        ("findings", "Findings"),
    ] {
        if let Some(arr) = v.get(key).and_then(|x| x.as_array()) {
            let _ = writeln!(s, "\n## {title} ({})", arr.len());
            for item in arr {
                let _ = writeln!(s, "- {}", summarize_health_item(item));
            }
        }
    }

    if let Some(arr) = v.get("warnings").and_then(|x| x.as_array())
        && !arr.is_empty()
    {
        let _ = writeln!(s, "\n## Warnings ({})", arr.len());
        for w in arr {
            let code = w.get("code").and_then(|x| x.as_str()).unwrap_or("");
            let msg = w.get("message").and_then(|x| x.as_str()).unwrap_or("");
            let _ = writeln!(s, "- [{code}] {msg}");
        }
    }

    s
}

/// Render a `{ key: count }` map as an indented sub-list under `title`,
/// skipping an empty/missing map. The empty-string schema key (an unpinned
/// mem) renders as `(unpinned)`.
fn render_count_map(s: &mut String, val: Option<&serde_json::Value>, title: &str) {
    use std::fmt::Write as _;
    let Some(map) = val.and_then(|x| x.as_object()) else {
        return;
    };
    if map.is_empty() {
        return;
    }
    let _ = writeln!(s, "- {title}:");
    for (k, n) in map {
        let label = if k.is_empty() { "(unpinned)" } else { k.as_str() };
        let _ = writeln!(s, "  - {label}: {}", n.as_u64().unwrap_or(0));
    }
}

/// One-line summary of a health detail item: prefer `id` (+ `title`),
/// else a dangling-link `from → target_id`, else the compact JSON.
fn summarize_health_item(item: &serde_json::Value) -> String {
    if let Some(id) = item.get("id").and_then(|x| x.as_str()) {
        match item.get("title").and_then(|x| x.as_str()) {
            Some(t) if !t.is_empty() => format!("{id} — {t}"),
            _ => id.to_string(),
        }
    } else if let Some(from) = item.get("from").and_then(|x| x.as_str()) {
        let target = item.get("target_id").and_then(|x| x.as_str()).unwrap_or("");
        format!("{from} → {target}")
    } else {
        serde_json::to_string(item).unwrap_or_default()
    }
}
