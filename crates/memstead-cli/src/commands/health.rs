use clap::Parser;
use serde_json::json;

use memstead_base::EntityId;
use memstead_base::Store;
use memstead_base::ops::{
    DanglingLink, HealthSummary, health::HEALTH_INCLUDE_KEYS,
    health::MissingRequiredOutgoingReport,
};

use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// Graph health summary.
///
/// Default: counts only. Pass `--include` to drill into details.
#[derive(Parser, Debug)]
pub struct Args {
    /// Opt heavy content into the response: orphans, stubs,
    /// most_connected, missing_fields, stale, dangling_links, tags,
    /// missing_required_outgoing, conformance, integrity. `conformance`
    /// lints every entity against the effective schema into a
    /// `findings` array (write-time typed codes); `integrity` adds the
    /// consistency axis (dangling links, stubs) to the same list.
    /// Repeatable (`--include K --include K`)
    /// AND comma-string (`--include K1,K2`) forms both parse — uniform
    /// with `memstead overview --include`.
    #[arg(long, value_delimiter = ',')]
    pub include: Vec<String>,

    /// Schema ref (`name@x.y.z`) the conformance/integrity includes
    /// lint against instead of each mem's current pin.
    #[arg(long)]
    pub target_schema: Option<String>,

    /// Max rows for `most_connected` and `tag_distribution` (default: 10).
    #[arg(long, default_value_t = 10)]
    pub limit: usize,

    /// Exit non-zero (1) when any included Tier-2 warning kind has
    /// present violations. The output is rendered first, then the
    /// non-zero exit fires. Today only `missing_required_outgoing`
    /// participates; new Tier-2 codes opt in additively without
    /// breaking the flag's semantics. With no Tier-2 `--include`
    /// token, `--strict` is a no-op.
    #[arg(long)]
    pub strict: bool,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let include = &args.include;
    // Tier-2 violation tally, populated as the corresponding `--include`
    // tokens are processed. Consulted at the end when `--strict` is set
    // to decide between exit 0 and exit 1. Per-code so a future
    // expansion (e.g. `cardinality_violations`) can list which codes
    // tripped without re-walking the report JSON.
    let mut strict_violations: Vec<(&'static str, usize)> = Vec::new();

    // Validate include-keys against the shared catalogue. Unknown keys
    // emit `UNKNOWN_INCLUDE_KEY` warnings the operator sees in both
    // markdown and JSON output — matches the MCP sibling's behaviour
    // and gives a typo zero-feedback path a typed signal instead.
    let mut include_warnings: Vec<(String, Vec<String>)> = Vec::new();
    for key in include {
        if !HEALTH_INCLUDE_KEYS.contains(&key.as_str()) {
            include_warnings.push((
                key.clone(),
                HEALTH_INCLUDE_KEYS.iter().map(|s| s.to_string()).collect(),
            ));
        }
    }

    let GatheredHealth {
        health,
        real_count,
        orphan_ids,
        stub_pairs,
        community_count,
        orphans_by_schema,
        communities_by_schema,
        most_connected_with_titles,
        missing_required_outgoing,
        tag_distribution,
        dangling_links,
        findings,
    } = match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(mut engine) => {
            let mut g = gather_mem_repo(&mut engine, args.limit, include);
            g.findings = gather_findings(&engine, include, args.target_schema.as_deref())?;
            g
        }
        CliEngine::Filesystem(mut engine) => {
            let mut g = gather_filesystem(&mut engine, args.limit, include);
            g.findings = gather_findings(&engine, include, args.target_schema.as_deref())?;
            g
        }
    };

    let mut result = json!({
        "summary": {
            "total_entities": real_count,
            "total_orphans": orphan_ids.len(),
            "total_stubs": stub_pairs.len(),
            "total_stale": health.stale_entities.len(),
            "total_missing_fields": health.missing_fields.len(),
            "total_communities": community_count,
            "orphans_by_schema": orphans_by_schema,
            "communities_by_schema": communities_by_schema,
        },
    });
    let obj = result.as_object_mut().unwrap();

    if include.iter().any(|s| s == "orphans") {
        let list: Vec<_> = orphan_ids
            .iter()
            .map(|(id, title)| json!({ "id": id.to_string(), "title": title }))
            .collect();
        obj.insert("orphans".into(), json!(list));
    }
    if include.iter().any(|s| s == "stubs") {
        let list: Vec<_> = stub_pairs
            .iter()
            .map(|(id, refs)| {
                json!({
                    "id": id.to_string(),
                    "referenced_by": refs.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                })
            })
            .collect();
        obj.insert("stubs".into(), json!(list));
    }
    if include.iter().any(|s| s == "most_connected") {
        let connected: Vec<_> = most_connected_with_titles
            .iter()
            .map(
                |(id, title, total, incoming, outgoing, typed_total, typed_incoming, typed_outgoing)| {
                    json!({
                        "id": id.to_string(),
                        "title": title,
                        "total": total,
                        "incoming": incoming,
                        "outgoing": outgoing,
                        "typed_total": typed_total,
                        "typed_incoming": typed_incoming,
                        "typed_outgoing": typed_outgoing,
                    })
                },
            )
            .collect();
        obj.insert("most_connected".into(), json!(connected));
    }
    if include.iter().any(|s| s == "missing_fields") {
        let list: Vec<_> = health
            .missing_fields
            .iter()
            .map(|h| {
                let missing: Vec<&str> = h.issues.iter().map(|i| i.field.as_str()).collect();
                json!({ "id": h.id.to_string(), "title": h.title, "missing": missing })
            })
            .collect();
        obj.insert("missing_fields".into(), json!(list));
    }
    if include.iter().any(|s| s == "stale") {
        let list: Vec<_> = health
            .stale_entities
            .iter()
            .map(|e| {
                json!({
                    "id": e.id.to_string(),
                    "title": e.title,
                    "days_since_modified": e.days_since_modified,
                })
            })
            .collect();
        obj.insert("stale".into(), json!(list));
    }
    if include.iter().any(|s| s == "missing_required_outgoing") {
        if !missing_required_outgoing.is_empty() {
            strict_violations.push(("missing_required_outgoing", missing_required_outgoing.len()));
        }
        obj.insert(
            "missing_required_outgoing".into(),
            serde_json::to_value(&missing_required_outgoing)?,
        );
    }
    if include.iter().any(|s| s == "dangling_links") {
        let arr: Vec<serde_json::Value> = dangling_links
            .iter()
            .map(|dl| serde_json::to_value(dl).unwrap_or(serde_json::Value::Null))
            .collect();
        obj.insert("dangling_links".into(), json!(arr));
    }
    if include.iter().any(|s| s == "conformance" || s == "integrity") {
        obj.insert("findings".into(), serde_json::to_value(&findings)?);
    }
    if include.iter().any(|s| s == "tags") {
        if let Some((distribution, folded, untagged)) = tag_distribution {
            obj.insert("tag_distribution".into(), distribution);
            obj.insert("tag_distribution_folded".into(), folded);
            obj.insert("untagged_entities".into(), untagged);
        }
    }

    // Typed warnings array — agents see `UNKNOWN_INCLUDE_KEY` here in
    // the same shape MCP emits on `warnings[]`. Empty when every key
    // resolved.
    if !include_warnings.is_empty() {
        let warning_payload: Vec<serde_json::Value> = include_warnings
            .iter()
            .map(|(key, allowed)| {
                json!({
                    "code": "UNKNOWN_INCLUDE_KEY",
                    "message": format!(
                        "unknown include key: \"{key}\". Allowed: {}",
                        allowed.join(", ")
                    ),
                    "details": { "key": key, "allowed": allowed },
                })
            })
            .collect();
        obj.insert("warnings".into(), json!(warning_payload));
    }

    if ctx.json {
        print_json(&result)?;
        return strict_exit(args.strict, &strict_violations);
    }

    // Markdown rendering
    let mut lines = Vec::new();
    lines.push("# Graph health".to_string());
    lines.push(String::new());
    lines.push(format!("- Entities: {real_count}"));
    if orphans_by_schema.len() > 1 {
        // Attribute the orphan headline per schema so by-design isolates
        // (ingest mems) aren't read as uniform debt.
        let by: Vec<String> = orphans_by_schema
            .iter()
            .map(|(s, n)| format!("{}: {n}", if s.is_empty() { "(unpinned)" } else { s }))
            .collect();
        lines.push(format!("- Orphans: {} ({})", orphan_ids.len(), by.join(", ")));
    } else {
        lines.push(format!("- Orphans: {}", orphan_ids.len()));
    }
    lines.push(format!("- Stubs: {}", stub_pairs.len()));
    lines.push(format!("- Stale: {}", health.stale_entities.len()));
    lines.push(format!("- Missing fields: {}", health.missing_fields.len()));
    lines.push(format!("- Communities: {community_count}"));
    lines.push(String::new());

    if let Some(v) = obj.get("orphans").and_then(|v| v.as_array()) {
        lines.push("## Orphans".to_string());
        for item in v {
            lines.push(format!(
                "- {} — {}",
                item["id"].as_str().unwrap_or(""),
                item["title"].as_str().unwrap_or("")
            ));
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("stubs").and_then(|v| v.as_array()) {
        lines.push("## Stubs".to_string());
        for item in v {
            lines.push(format!("- {}", item["id"].as_str().unwrap_or("")));
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("most_connected").and_then(|v| v.as_array()) {
        lines.push("## Most connected".to_string());
        lines.push("(ranked by typed dependency degree; total keeps mention edges)".to_string());
        for item in v {
            lines.push(format!(
                "- {} — {} (typed {}, total {}, in {}, out {})",
                item["id"].as_str().unwrap_or(""),
                item["title"].as_str().unwrap_or(""),
                item["typed_total"].as_u64().unwrap_or(0),
                item["total"].as_u64().unwrap_or(0),
                item["incoming"].as_u64().unwrap_or(0),
                item["outgoing"].as_u64().unwrap_or(0),
            ));
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("missing_fields").and_then(|v| v.as_array()) {
        lines.push("## Missing fields".to_string());
        for item in v {
            let missing: Vec<&str> = item["missing"]
                .as_array()
                .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
                .unwrap_or_default();
            lines.push(format!(
                "- {} — {} (missing: {})",
                item["id"].as_str().unwrap_or(""),
                item["title"].as_str().unwrap_or(""),
                missing.join(", ")
            ));
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("stale").and_then(|v| v.as_array()) {
        lines.push("## Stale entities".to_string());
        for item in v {
            lines.push(format!(
                "- {} — {} ({} days)",
                item["id"].as_str().unwrap_or(""),
                item["title"].as_str().unwrap_or(""),
                item["days_since_modified"].as_u64().unwrap_or(0)
            ));
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("missing_required_outgoing").and_then(|v| v.as_array()) {
        lines.push("## Missing required outgoing".to_string());
        for item in v {
            let blocks: Vec<String> = item["missing"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|b| {
                            let rels: Vec<&str> = b["relationships"]
                                .as_array()
                                .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
                                .unwrap_or_default();
                            format!(
                                "[{}] {}",
                                rels.join(", "),
                                b["cardinality"].as_str().unwrap_or("")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            lines.push(format!(
                "- {} — {} (missing: {})",
                item["id"].as_str().unwrap_or(""),
                item["title"].as_str().unwrap_or(""),
                blocks.join("; ")
            ));
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("dangling_links").and_then(|v| v.as_array()) {
        lines.push("## Dangling links".to_string());
        for item in v {
            lines.push(format!(
                "- {} → {} (section: {})",
                item["from"].as_str().unwrap_or(""),
                item["target_id"].as_str().unwrap_or(""),
                item["section"].as_str().unwrap_or("(none)")
            ));
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("tag_distribution").and_then(|v| v.as_array()) {
        lines.push("## Tags".to_string());
        for item in v {
            lines.push(format!(
                "- {} ({})",
                item["tag"].as_str().unwrap_or(""),
                item["count"].as_u64().unwrap_or(0)
            ));
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("warnings").and_then(|v| v.as_array()) {
        lines.push("## Warnings".to_string());
        for w in v {
            lines.push(format!(
                "- {} — {}",
                w["code"].as_str().unwrap_or(""),
                w["message"].as_str().unwrap_or("")
            ));
        }
        lines.push(String::new());
    }
    if let Some(u) = obj.get("untagged_entities") {
        lines.push("## Untagged".to_string());
        lines.push(format!("- Total: {}", u["total"].as_u64().unwrap_or(0)));
        if let Some(by_type) = u["by_entity_type"].as_object() {
            let mut entries: Vec<(&String, u64)> = by_type
                .iter()
                .map(|(k, v)| (k, v.as_u64().unwrap_or(0)))
                .collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            for (kind, count) in entries {
                lines.push(format!("  - {kind}: {count}"));
            }
        }
        lines.push(String::new());
    }

    print_markdown(&lines.join("\n"));
    strict_exit(args.strict, &strict_violations)
}

/// Aggregated health data, engine-flavour-agnostic. Both
/// One `most_connected` row resolved at gather time:
/// `(id, title, total, incoming, outgoing, typed_total, typed_incoming,
/// typed_outgoing)`. `typed_*` excludes auto-emitted mention edges so the
/// ranking reflects dependency, not co-mention.
type MostConnectedRow = (EntityId, String, usize, usize, usize, usize, usize, usize);

/// mem-repo and filesystem gather paths populate this struct
/// with the same shape so the rendering / JSON-envelope code below
/// runs once.
struct GatheredHealth {
    health: HealthSummary,
    /// Integrity findings (`{id, axis, code, detail}`) — populated by
    /// the caller (engine-shaped, so outside `gather_from_store`) when
    /// `--include conformance` / `--include integrity` is requested.
    findings: Vec<memstead_base::ops::integrity::IntegrityFinding>,
    real_count: usize,
    /// `(id, title)` pairs — title resolved at gather time so the
    /// rendering layer doesn't need to keep the engine alive.
    orphan_ids: Vec<(EntityId, String)>,
    stub_pairs: Vec<(EntityId, Vec<EntityId>)>,
    community_count: usize,
    /// #49: orphan/community counts attributed per pinned schema, so a
    /// blended headline isn't read as uniform debt (ingest-mem isolates
    /// are orphans by design; code-mem orphans are debt). Filled by the
    /// engine-aware gather wrappers — `gather_from_store` leaves them empty.
    orphans_by_schema: std::collections::BTreeMap<String, usize>,
    communities_by_schema: std::collections::BTreeMap<String, usize>,
    /// [`MostConnectedRow`] tuples — same reasoning as `orphan_ids`.
    most_connected_with_titles: Vec<MostConnectedRow>,
    missing_required_outgoing: Vec<MissingRequiredOutgoingReport>,
    /// `Some(...)` when the caller asked for `--include tags`,
    /// `None` otherwise. The triple is `(distribution, folded,
    /// untagged)` mirroring `collect_tag_distribution`'s return
    /// shape.
    /// Pre-serialised tag triple: `(distribution, folded, untagged)`
    /// already converted to `serde_json::Value`. Keeps the gather
    /// step engine-flavour-agnostic without exposing the
    /// `memstead_base::ops::health` private tag types through this
    /// crate's public surface.
    tag_distribution: Option<(serde_json::Value, serde_json::Value, serde_json::Value)>,
    /// Populated when `--include dangling_links` is set; empty
    /// otherwise. Matches the MCP `memstead_health` tool's response
    /// shape — `{from, target_id, target_path, section}` per entry.
    dangling_links: Vec<DanglingLink>,
}

/// Conformance/integrity findings across every mounted mem, in
/// sorted mem order. Engine-shaped (needs schema resolution), so it
/// runs beside `gather_from_store`, not inside it. `target_schema`
/// parse and resolution failures surface as typed CLI errors — the
/// same codes the MCP surface refuses with.
fn gather_findings(
    engine: &memstead_base::Engine,
    include: &[String],
    target_schema: Option<&str>,
) -> anyhow::Result<Vec<memstead_base::ops::integrity::IntegrityFinding>> {
    let wants_conformance = include
        .iter()
        .any(|s| s == "conformance" || s == "integrity");
    if !wants_conformance {
        return Ok(Vec::new());
    }
    let target: Option<memstead_schema::SchemaRef> = match target_schema {
        None => None,
        Some(raw) => Some(
            raw.parse::<memstead_schema::SchemaRef>()
                .map_err(|reason| anyhow::anyhow!("invalid --target-schema {raw:?}: {reason}"))?,
        ),
    };
    let mut mems: Vec<String> = engine.schemas().keys().cloned().collect();
    mems.sort();
    let mut findings = Vec::new();
    for v in &mems {
        findings.extend(
            engine
                .conformance_findings(v, target.as_ref())
                .map_err(crate::CliError::from_engine_op)?,
        );
        if include.iter().any(|s| s == "integrity") {
            findings.extend(
                engine
                    .consistency_findings(v)
                    .map_err(crate::CliError::from_engine_op)?,
            );
        }
    }
    Ok(findings)
}

#[cfg(feature = "mem-repo")]
fn gather_mem_repo(
    engine: &mut memstead_base::Engine,
    limit: usize,
    include: &[String],
) -> GatheredHealth {
    let mut g = gather_from_store(
        engine.health(),
        engine.store(),
        engine.communities().count,
        limit,
        include,
        |limit| engine_most_connected_mem_repo(engine, limit),
        || engine.missing_required_outgoing(None),
    );
    fill_schema_breakdowns(engine, &mut g);
    g
}

fn gather_filesystem(
    engine: &mut memstead_base::Engine,
    limit: usize,
    include: &[String],
) -> GatheredHealth {
    let mut g = gather_from_store(
        engine.health(),
        engine.store(),
        engine.communities().count,
        limit,
        include,
        |limit| engine_most_connected_filesystem(engine, limit),
        || engine.missing_required_outgoing(None),
    );
    fill_schema_breakdowns(engine, &mut g);
    g
}

/// #49: attribute the orphan / community headlines per pinned schema (the
/// engine-aware step `gather_from_store` can't do off a bare `&Store`).
fn fill_schema_breakdowns(engine: &memstead_base::Engine, g: &mut GatheredHealth) {
    let mems: Vec<String> = engine.mounts().iter().map(|m| m.mem.clone()).collect();
    g.orphans_by_schema = engine.orphans_by_schema(&engine.orphans());
    g.communities_by_schema = engine.communities_by_schema(&mems);
}

/// Engine-agnostic gather pipeline. The two engine-shaped callbacks
/// (`most_connected_fn`, `missing_required_outgoing_fn`) handle the
/// surfaces that are not available off the bare `&Store`.
fn gather_from_store(
    health: HealthSummary,
    store: &Store,
    community_count: usize,
    limit: usize,
    include: &[String],
    most_connected_fn: impl FnOnce(usize) -> Vec<MostConnectedRow>,
    missing_required_outgoing_fn: impl FnOnce() -> Vec<MissingRequiredOutgoingReport>,
) -> GatheredHealth {
    let real_count = store.all_entities().filter(|e| !e.stub).count();
    let orphan_ids: Vec<(EntityId, String)> = memstead_base::graph::query::find_orphans(store)
        .into_iter()
        .map(|id| {
            let title = store.get(&id).map(|e| e.title.clone()).unwrap_or_default();
            (id, title)
        })
        .collect();
    let stub_pairs = memstead_base::graph::query::find_stubs(store);
    let most_connected_with_titles = if include.iter().any(|s| s == "most_connected") {
        most_connected_fn(limit)
    } else {
        Vec::new()
    };
    let missing_required_outgoing = if include.iter().any(|s| s == "missing_required_outgoing") {
        missing_required_outgoing_fn()
    } else {
        Vec::new()
    };
    let tag_distribution = if include.iter().any(|s| s == "tags") {
        let (distribution, folded, untagged) =
            memstead_base::ops::health::collect_tag_distribution(store, None, limit);
        Some((
            serde_json::to_value(&distribution).unwrap_or(serde_json::Value::Null),
            serde_json::to_value(&folded).unwrap_or(serde_json::Value::Null),
            serde_json::to_value(&untagged).unwrap_or(serde_json::Value::Null),
        ))
    } else {
        None
    };
    let dangling_links = if include.iter().any(|s| s == "dangling_links") {
        memstead_base::ops::health::collect_dangling_links(store, None)
    } else {
        Vec::new()
    };
    GatheredHealth {
        health,
        findings: Vec::new(),
        real_count,
        orphan_ids,
        stub_pairs,
        community_count,
        // Engine-agnostic path can't resolve schema pins; the engine-aware
        // wrappers (`gather_mem_repo` / `gather_filesystem`) fill these.
        orphans_by_schema: std::collections::BTreeMap::new(),
        communities_by_schema: std::collections::BTreeMap::new(),
        most_connected_with_titles,
        missing_required_outgoing,
        tag_distribution,
        dangling_links,
    }
}

#[cfg(feature = "mem-repo")]
fn engine_most_connected_mem_repo(
    engine: &memstead_base::Engine,
    limit: usize,
) -> Vec<MostConnectedRow> {
    engine
        .most_connected(limit)
        .into_iter()
        .map(|c| {
            let title = engine
                .get_entity(&c.id)
                .map(|e| e.title.clone())
                .unwrap_or_default();
            (
                c.id,
                title,
                c.total,
                c.incoming,
                c.outgoing,
                c.typed_total,
                c.typed_incoming,
                c.typed_outgoing,
            )
        })
        .collect()
}

fn engine_most_connected_filesystem(
    engine: &memstead_base::Engine,
    limit: usize,
) -> Vec<MostConnectedRow> {
    engine
        .most_connected(limit)
        .into_iter()
        .map(|c| {
            let title = engine
                .get_entity(&c.id)
                .map(|e| e.title.clone())
                .unwrap_or_default();
            (
                c.id,
                title,
                c.total,
                c.incoming,
                c.outgoing,
                c.typed_total,
                c.typed_incoming,
                c.typed_outgoing,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn help_lists_every_include_key() {
        let cmd = Args::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "include")
            .expect("--include arg must exist");
        let help = arg
            .get_help()
            .expect("--include must have help text")
            .to_string();
        for key in HEALTH_INCLUDE_KEYS {
            assert!(
                help.contains(key),
                "`memstead health --help` must name include key `{key}` (got: {help})"
            );
        }
    }
}

/// Translate the strict-violation tally into an exit code. With
/// `--strict` set and any Tier-2 violations recorded, return a
/// `CliError(Generic)` so `main` exits 1 after the report has been
/// written to stdout. When `--strict` is unset, or when no Tier-2
/// `--include` token was supplied, this is a no-op.
fn strict_exit(strict: bool, violations: &[(&'static str, usize)]) -> anyhow::Result<()> {
    if !strict || violations.is_empty() {
        return Ok(());
    }
    let summary = violations
        .iter()
        .map(|(code, n)| format!("{code}: {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(crate::CliError::new(
        ExitKind::Generic,
        "HEALTH_STRICT_VIOLATIONS",
        format!("strict mode: tier-2 violations present ({summary})"),
    )
    .into())
}
