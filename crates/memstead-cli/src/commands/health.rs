use clap::Parser;
use serde_json::json;

use memstead_base::EntityId;
use memstead_base::Store;
use memstead_base::ops::{
    DanglingLink, HealthSummary, health::ConstraintFindingReport, health::HEALTH_INCLUDE_KEYS,
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
    /// missing_required_outgoing, constraints (standing violations of
    /// declared schema constraints), conformance, integrity, config,
    /// anchors (per-mem counts of the four standalone
    /// anchor-verification states), friction (the workspace-local
    /// refusal ledger's summary — counts per typed refusal code and
    /// per verb, whole-ledger plus a recent 24h window; local-only,
    /// content-free), open_questions (per-mem composed worklist of
    /// what the holding does not know: stubs, recheck/unresolvable
    /// anchors, unsatisfied constraints, dangling links, and a paired
    /// process mem's open entries — negative findings separated as
    /// already-searched; capped per kind with an explicit `more`
    /// count), stale_derivations (per-mem derivation edges whose
    /// target changed since the recorded baseline, plus unbaselined
    /// edges — re-assert via `memstead relate` to refresh), checks
    /// (per-mem counts of the four derived check states plus the
    /// author≠checker independence gate: self_checked /
    /// confirmed_independent / unconfirmable — transport is not
    /// identity, so until a caller-declared identity exists every
    /// ok-checked entity reports unconfirmable; the other two
    /// categories are explicit empties).
    /// `conformance` lints every entity against the effective schema
    /// into a `findings` array (write-time typed codes); `integrity`
    /// adds the consistency axis (dangling links, stubs) to the same
    /// list. `config` renders the workspace-config projection (per-mem
    /// origin/storage/vcs detail, `mutations`, `plugin`) — the same
    /// block MCP's `include_config: true` serves.
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
    /// present violations, or when the always-on authoring-drift axis
    /// reports findings (`SCHEMA_AUTHORING_SOURCE_MISSING` /
    /// `SCHEMA_AUTHORING_SOURCE_DIVERGED` — no `--include` opt-in).
    /// The output is rendered first, then the non-zero exit fires.
    /// Include-gated participation today: `missing_required_outgoing`, `constraints`;
    /// new Tier-2 codes opt in additively without breaking the flag's
    /// semantics.
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
        constraint_findings,
        schema_format_defects,
        tag_distribution,
        dangling_links,
        findings,
        config_entries,
        anchors_axis,
        open_questions_axis,
        stale_derivations_axis,
        checks_axis,
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
                |(
                    id,
                    title,
                    total,
                    incoming,
                    outgoing,
                    typed_total,
                    typed_incoming,
                    typed_outgoing,
                )| {
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
                // `missing` (bare field names) stays byte-identical for
                // existing consumers; the per-issue detail rides next to
                // it so the CLI projection carries WHICH condition each
                // issue reports — same additive shape as the MCP
                // composer's.
                let missing: Vec<&str> = h.issues.iter().map(|i| i.field.as_str()).collect();
                let issues: Vec<_> = h
                    .issues
                    .iter()
                    .map(|i| json!({ "field": i.field, "code": i.code, "message": i.message }))
                    .collect();
                json!({
                    "id": h.id.to_string(),
                    "title": h.title,
                    "missing": missing,
                    "issues": issues,
                })
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
    if include.iter().any(|s| s == "constraints") {
        if !constraint_findings.is_empty() {
            strict_violations.push(("constraints", constraint_findings.len()));
        }
        obj.insert(
            "constraints".into(),
            serde_json::to_value(&constraint_findings)?,
        );
        // Defective section-format declarations (lenient boot):
        // additive key, present only when defects exist.
        if !schema_format_defects.is_empty() {
            strict_violations.push(("schema_format_defects", schema_format_defects.len()));
            obj.insert(
                "schema_format_defects".into(),
                serde_json::to_value(&schema_format_defects)?,
            );
        }
    }
    if include.iter().any(|s| s == "dangling_links") {
        let arr: Vec<serde_json::Value> = dangling_links
            .iter()
            .map(|dl| serde_json::to_value(dl).unwrap_or(serde_json::Value::Null))
            .collect();
        obj.insert("dangling_links".into(), json!(arr));
    }
    if include
        .iter()
        .any(|s| s == "conformance" || s == "integrity")
    {
        obj.insert("findings".into(), serde_json::to_value(&findings)?);
    }
    if include.iter().any(|s| s == "tags")
        && let Some((distribution, folded, untagged)) = tag_distribution
    {
        obj.insert("tag_distribution".into(), distribution);
        obj.insert("tag_distribution_folded".into(), folded);
        obj.insert("untagged_entities".into(), untagged);
    }
    // `--include config`: the shared workspace-config projection
    // (`mems` / `mutations` / `plugin`), rendered by the same
    // implementation MCP's `include_config: true` uses.
    if let Some(entries) = config_entries {
        for (k, v) in entries {
            obj.insert(k, v);
        }
    }
    if let Some(axis) = &anchors_axis {
        obj.insert("anchors".to_string(), axis.clone());
    }
    if let Some(axis) = &open_questions_axis {
        obj.insert("open_questions".to_string(), axis.clone());
    }
    if let Some(axis) = &stale_derivations_axis {
        obj.insert("stale_derivations".to_string(), axis.clone());
    }
    if let Some(axis) = &checks_axis {
        obj.insert("checks".to_string(), axis.clone());
    }
    // `--include friction`: the friction ledger's read surface
    // (agent-trust plan 08) — counts per refusal code / per verb,
    // whole ledger plus a recent 24h window. Same summarizer MCP's
    // axis serves; no workspace resolvable → empty summary.
    let friction_axis = if include.iter().any(|s| s == "friction") {
        let summary = std::env::current_dir()
            .ok()
            .and_then(|cwd| crate::setup::find_workspace_root(&cwd))
            .map(|root| memstead_base::friction::FrictionLedger::for_workspace(&root).summarize())
            .unwrap_or_else(|| {
                json!({
                    "total": 0,
                    "by_code": {},
                    "by_verb": {},
                    "recent_24h": { "total": 0, "by_code": {} },
                    "ledger_bytes": 0,
                })
            });
        obj.insert("friction".to_string(), summary.clone());
        Some(summary)
    } else {
        None
    };

    // Typed warnings array — engine-level health warnings (load-time
    // drift, the authoring-drift axis, …) in the same `{code, message,
    // details}` shape MCP emits on `warnings[]`, plus any
    // `UNKNOWN_INCLUDE_KEY` request warnings. Previously the CLI
    // rendered only the include-key warnings, leaving engine warnings
    // MCP-only — the blindness the authoring-drift axis exists to fix
    // was measured through exactly this gap.
    let mut warning_payload: Vec<serde_json::Value> = health
        .warnings
        .iter()
        .filter_map(|w| serde_json::to_value(w).ok())
        .collect();
    warning_payload.extend(include_warnings.iter().map(|(key, allowed)| {
        json!({
            "code": "UNKNOWN_INCLUDE_KEY",
            "message": format!(
                "unknown include key: \"{key}\". Allowed: {}",
                allowed.join(", ")
            ),
            "details": { "key": key, "allowed": allowed },
        })
    }));
    if !warning_payload.is_empty() {
        obj.insert("warnings".into(), json!(warning_payload));
    }
    // Leaf populations — the counts the orphan axis exempts because
    // those types are terminal by construction (agent-trust plan 06).
    if !health.leaf_entities_by_type.is_empty() {
        obj.insert(
            "leaf_entities_by_type".into(),
            serde_json::to_value(&health.leaf_entities_by_type).unwrap_or_default(),
        );
    }
    // Quarantine roster — a boot-honesty fact, present whenever
    // non-empty, never behind an include gate (agent-trust plan 04).
    if !health.quarantined.is_empty() {
        obj.insert(
            "quarantined".into(),
            serde_json::to_value(&health.quarantined).unwrap_or_default(),
        );
    }
    if let Some(diag) = &health.boot_diagnosis {
        obj.insert("boot_diagnosis".into(), diag.clone());
    }

    // Authoring-drift findings participate in `--strict`
    // unconditionally (no `--include` opt-in): they are
    // default-visible warnings, and the axis exists because a
    // `health --strict` run stayed silent on a vanished authoring
    // source.
    let authoring_drift = health
        .warnings
        .iter()
        .filter(|w| {
            matches!(
                w.code(),
                "SCHEMA_AUTHORING_SOURCE_MISSING" | "SCHEMA_AUTHORING_SOURCE_DIVERGED"
            )
        })
        .count();
    if authoring_drift > 0 {
        strict_violations.push(("schema_authoring_drift", authoring_drift));
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
        lines.push(format!(
            "- Orphans: {} ({})",
            orphan_ids.len(),
            by.join(", ")
        ));
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
            // Render per-issue `field (CODE)` so a heading mismatch never
            // reads as "missing" to a human either — content under a
            // non-deriving heading EXISTS; the label must say which
            // condition fired. Falls back to the legacy field-name list
            // for payloads without `issues` (older JSON piped back in).
            let labels: Vec<String> = match item["issues"].as_array() {
                Some(issues) if !issues.is_empty() => issues
                    .iter()
                    .map(|i| {
                        format!(
                            "{} ({})",
                            i["field"].as_str().unwrap_or(""),
                            i["code"].as_str().unwrap_or("MISSING"),
                        )
                    })
                    .collect(),
                _ => item["missing"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            lines.push(format!(
                "- {} — {} (issues: {})",
                item["id"].as_str().unwrap_or(""),
                item["title"].as_str().unwrap_or(""),
                labels.join(", ")
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
    if let Some(v) = obj
        .get("missing_required_outgoing")
        .and_then(|v| v.as_array())
    {
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

    if let Some(axis) = anchors_axis.as_ref().and_then(|a| a.as_object()) {
        lines.push(format!("## Anchors ({} mems)", axis.len()));
        for (mem, counts) in axis {
            lines.push(format!(
                "- `{mem}`: resolved {}, drifted {}, recheck {}, unresolvable {}",
                counts["resolved"].as_u64().unwrap_or(0),
                counts["drifted"].as_u64().unwrap_or(0),
                counts["recheck"].as_u64().unwrap_or(0),
                counts["unresolvable"].as_u64().unwrap_or(0),
            ));
        }
        lines.push(String::new());
    }

    if let Some(axis) = open_questions_axis.as_ref().and_then(|a| a.as_object()) {
        let cap = axis
            .get("_item_cap")
            .and_then(|v| v.as_u64())
            .unwrap_or_default();
        lines.push(format!("## Open questions (item cap {cap} per kind)"));
        for (mem, entry) in axis.iter().filter(|(k, _)| *k != "_item_cap") {
            let total = entry["total_open"].as_u64().unwrap_or(0);
            lines.push(format!("- `{mem}`: {total} open"));
            for kind in [
                "stubs",
                "anchors_recheck",
                "anchors_unresolvable",
                "unsatisfied_constraints",
                "dangling_links",
            ] {
                let count = entry[kind]["count"].as_u64().unwrap_or(0);
                if count > 0 {
                    let more = entry[kind]["more"].as_u64().unwrap_or(0);
                    let suffix = if more > 0 {
                        format!(" ({more} more not shown)")
                    } else {
                        String::new()
                    };
                    lines.push(format!("  - {kind}: {count}{suffix}"));
                }
            }
            if let Some(process) = entry.get("process").and_then(|p| p.as_array()) {
                for p in process {
                    if p["resolvable"] == serde_json::json!(true) {
                        lines.push(format!(
                            "  - process `{}`: {} open entries; {} already searched (do not redo)",
                            p["binding"].as_str().unwrap_or("?"),
                            p["open_entries"]["count"].as_u64().unwrap_or(0),
                            p["already_searched"]["count"].as_u64().unwrap_or(0),
                        ));
                    } else {
                        lines.push(format!(
                            "  - process `{}`: not resolvable (mem not mounted)",
                            p["binding"].as_str().unwrap_or("?"),
                        ));
                    }
                }
            }
        }
        lines.push(String::new());
    }

    // Checks axis — same wording as the MCP text renderer
    // (`render_health_markdown`). Null-is-a-statement: requested with
    // no mems renders the explicit zero heading; not requested
    // renders nothing.
    if let Some(axis) = checks_axis.as_ref().and_then(|a| a.as_object()) {
        lines.push(format!("## Checks ({} mems)", axis.len()));
        for (mem, c) in axis {
            let count = |key: &str| c.get(key).and_then(|x| x.as_u64()).unwrap_or(0);
            let gate = |key: &str| {
                c.get("independence")
                    .and_then(|g| g.get(key))
                    .and_then(|e| e.get("count"))
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0)
            };
            lines.push(format!(
                "- `{mem}`: never_checked {}, checked_ok {}, check_failed {}, \
                 check_stale {}; independence: self_checked {}, \
                 confirmed_independent {}, unconfirmable {}",
                count("never_checked"),
                count("checked_ok"),
                count("check_failed"),
                count("check_stale"),
                gate("self_checked"),
                gate("confirmed_independent"),
                gate("unconfirmable"),
            ));
        }
        lines.push(String::new());
    }

    // Stale-derivations axis — same requested-vs-absent contract and
    // wording as the MCP text renderer.
    if let Some(axis) = stale_derivations_axis.as_ref().and_then(|a| a.as_object()) {
        let total: usize = axis
            .values()
            .filter_map(|a| a.as_array().map(|a| a.len()))
            .sum();
        lines.push(format!("## Stale derivations ({total} findings)"));
        for (mem, findings) in axis {
            for f in findings.as_array().into_iter().flatten() {
                lines.push(format!(
                    "- `{mem}`: {} -[{}]-> {} ({})",
                    f.get("source").and_then(|x| x.as_str()).unwrap_or(""),
                    f.get("rel_type").and_then(|x| x.as_str()).unwrap_or(""),
                    f.get("target").and_then(|x| x.as_str()).unwrap_or(""),
                    f.get("state").and_then(|x| x.as_str()).unwrap_or(""),
                ));
            }
        }
        lines.push(String::new());
    }

    // Quarantine roster — ungated (present in the JSON whenever
    // non-empty), so the markdown renders it whenever present: per
    // mem the reason code plus the message, which carries the repair
    // command.
    if let Some(arr) = obj.get("quarantined").and_then(|v| v.as_array()) {
        lines.push(format!("## Quarantined mems ({})", arr.len()));
        for q in arr {
            lines.push(format!(
                "- `{}` [{}] {}",
                q.get("mem").and_then(|x| x.as_str()).unwrap_or(""),
                q.get("reason_code").and_then(|x| x.as_str()).unwrap_or(""),
                q.get("reason_message")
                    .and_then(|x| x.as_str())
                    .unwrap_or(""),
            ));
        }
        lines.push(String::new());
    }

    if let Some(f) = &friction_axis {
        lines.push(format!(
            "## Friction ({} refusals recorded, {} in the last 24h)",
            f["total"].as_u64().unwrap_or(0),
            f["recent_24h"]["total"].as_u64().unwrap_or(0),
        ));
        if let Some(by_code) = f["by_code"].as_object().filter(|m| !m.is_empty()) {
            lines.push("- by code:".to_string());
            let mut entries: Vec<(&String, u64)> = by_code
                .iter()
                .map(|(k, v)| (k, v.as_u64().unwrap_or(0)))
                .collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            for (code, count) in entries {
                lines.push(format!("  - {code}: {count}"));
            }
        }
        if let Some(by_verb) = f["by_verb"].as_object().filter(|m| !m.is_empty()) {
            lines.push("- by verb:".to_string());
            let mut entries: Vec<(&String, u64)> = by_verb
                .iter()
                .map(|(k, v)| (k, v.as_u64().unwrap_or(0)))
                .collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            for (verb, count) in entries {
                lines.push(format!("  - {verb}: {count}"));
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
    /// Standing violations of declared schema `constraints`
    /// (`--include constraints`), empty otherwise.
    constraint_findings: Vec<ConstraintFindingReport>,
    /// Defective section-format declarations the loaded schemas carry
    /// (rides the `constraints` include), empty otherwise.
    schema_format_defects: Vec<memstead_base::ops::health::SchemaFormatDefect>,
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
    /// `Some(...)` when the caller asked for `--include config`: the
    /// same top-level entries (`mems`, `mutations`, `plugin`) the MCP
    /// composer renders for `include_config: true`, produced by the
    /// shared `memstead_base::ops::health::config_projection` with the
    /// policy values derived from `Engine::settings()`. `None`
    /// otherwise — absence of the key means "not requested".
    config_entries: Option<serde_json::Map<String, serde_json::Value>>,
    /// `Some(...)` when the caller asked for `--include anchors`: the
    /// per-mem four-state counts from the shared
    /// `health_anchors_axis` helper (same axis MCP renders). `None`
    /// otherwise — absence of the key means "not requested".
    anchors_axis: Option<serde_json::Value>,
    /// `Some(...)` when the caller asked for `--include
    /// open_questions`: the composed per-mem worklist from the shared
    /// `health_open_questions_axis` helper (same axis MCP renders).
    open_questions_axis: Option<serde_json::Value>,
    /// `Some(...)` when the caller asked for `--include
    /// stale_derivations`: per-mem derivation-staleness findings from
    /// the shared `health_stale_derivations_axis` helper.
    stale_derivations_axis: Option<serde_json::Value>,
    /// `--include checks` — per-mem check-state counts + the
    /// author≠checker independence gate, via the shared
    /// `health_checks_axis` helper.
    checks_axis: Option<serde_json::Value>,
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
        || engine.orphans(),
        |limit| engine_most_connected_mem_repo(engine, limit),
        || engine.missing_required_outgoing(None),
        || engine.constraint_findings(None),
        || engine.schema_format_defects(),
    );
    fill_schema_breakdowns(engine, &mut g);
    fill_config_projection(engine, include, &mut g);
    fill_anchors_axis(engine, include, &mut g);
    fill_open_questions_axis(engine, include, &mut g);
    fill_stale_derivations_axis(engine, include, &mut g);
    fill_checks_axis(engine, include, &mut g);
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
        || engine.orphans(),
        |limit| engine_most_connected_filesystem(engine, limit),
        || engine.missing_required_outgoing(None),
        || engine.constraint_findings(None),
        || engine.schema_format_defects(),
    );
    fill_schema_breakdowns(engine, &mut g);
    fill_config_projection(engine, include, &mut g);
    fill_anchors_axis(engine, include, &mut g);
    fill_open_questions_axis(engine, include, &mut g);
    fill_stale_derivations_axis(engine, include, &mut g);
    fill_checks_axis(engine, include, &mut g);
    g
}

/// #49: attribute the orphan / community headlines per pinned schema (the
/// engine-aware step `gather_from_store` can't do off a bare `&Store`).
/// Engine-aware step for `--include config` — renders the shared
/// workspace-config projection (one implementation with the MCP
/// composer) off the engine's own settings.
fn fill_config_projection(
    engine: &memstead_base::Engine,
    include: &[String],
    g: &mut GatheredHealth,
) {
    if include.iter().any(|s| s == "config") {
        let mut mems: Vec<String> = engine
            .mem_router()
            .writable_mems()
            .iter()
            .cloned()
            .collect();
        mems.sort();
        let (mutations, plugin) =
            memstead_base::ops::health::config_projection_from_settings(engine.settings());
        g.config_entries = Some(memstead_base::ops::health::config_projection(
            engine, &mems, mutations, plugin,
        ));
    }
}

/// Engine-aware step for `--include anchors` — the per-mem four-state
/// counts from the shared axis helper.
/// Engine-aware step for `--include open_questions` — the composed
/// what-don't-we-know worklist (agent-trust plan 11), one shared
/// implementation with the MCP composer.
fn fill_open_questions_axis(
    engine: &memstead_base::Engine,
    include: &[String],
    g: &mut GatheredHealth,
) {
    if include.iter().any(|s| s == "open_questions") {
        g.open_questions_axis = Some(
            memstead_base::ops::health::health_open_questions_axis(engine, None),
        );
    }
}

/// Engine-aware step for `--include stale_derivations` — per-mem
/// derivation-staleness findings (agent-trust plan 12), one shared
/// implementation with the MCP composer.
fn fill_stale_derivations_axis(
    engine: &memstead_base::Engine,
    include: &[String],
    g: &mut GatheredHealth,
) {
    if include.iter().any(|s| s == "stale_derivations") {
        g.stale_derivations_axis = Some(
            memstead_base::ops::health::health_stale_derivations_axis(engine, None),
        );
    }
}

fn fill_checks_axis(engine: &memstead_base::Engine, include: &[String], g: &mut GatheredHealth) {
    if include.iter().any(|s| s == "checks") {
        g.checks_axis = Some(memstead_base::ops::health::health_checks_axis(engine, None));
    }
}

fn fill_anchors_axis(engine: &memstead_base::Engine, include: &[String], g: &mut GatheredHealth) {
    if include.iter().any(|s| s == "anchors") {
        g.anchors_axis = Some(memstead_base::ops::health::health_anchors_axis(engine));
    }
}

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
    orphans_fn: impl FnOnce() -> Vec<EntityId>,
    most_connected_fn: impl FnOnce(usize) -> Vec<MostConnectedRow>,
    missing_required_outgoing_fn: impl FnOnce() -> Vec<MissingRequiredOutgoingReport>,
    constraint_findings_fn: impl FnOnce() -> Vec<ConstraintFindingReport>,
    schema_format_defects_fn: impl FnOnce() -> Vec<memstead_base::ops::health::SchemaFormatDefect>,
) -> GatheredHealth {
    let real_count = store.all_entities().filter(|e| !e.stub).count();
    let orphan_ids: Vec<(EntityId, String)> = orphans_fn()
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
    let constraint_findings = if include.iter().any(|s| s == "constraints") {
        constraint_findings_fn()
    } else {
        Vec::new()
    };
    let schema_format_defects = if include.iter().any(|s| s == "constraints") {
        schema_format_defects_fn()
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
        constraint_findings,
        schema_format_defects,
        tag_distribution,
        dangling_links,
        config_entries: None,
        anchors_axis: None,
        open_questions_axis: None,
        stale_derivations_axis: None,
        checks_axis: None,
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
