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
    /// anchors (per-mem counts of the standalone anchor-verification
    /// states, with `unresolvable` meaning the artifact is GONE and
    /// `unobserved` meaning the pass could not measure it, plus the
    /// population those counts cover), ledger (a FOLDER mem's change
    /// ledger set against the markdown files beside it: entities the
    /// ledger records with no file, and files the ledger never
    /// mentions — read-only, it never writes or tidies a ledger line;
    /// git-branch mems are absent rather than clean, because their
    /// change set is a real two-tree diff and the divergence cannot
    /// arise), friction (the workspace-local
    /// refusal ledger's summary — counts per typed refusal code and
    /// per verb, with per-code reason breakdowns where the code
    /// carries a closed engine-owned discriminator, whole-ledger plus
    /// a recent 24h window; local-only, values drawn from closed
    /// engine-defined vocabularies only), open_questions (per-mem
    /// composed worklist of
    /// what the holding does not know: stubs, anchors that are recheck,
    /// unresolvable (artifact gone), unobserved (not measured) or
    /// dangling (entity gone), unsatisfied constraints, dangling links,
    /// and a paired
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
    /// categories are explicit empties), signals (entities whose
    /// declared aggregate signals sit above `none`, each with value,
    /// level and contributing entity ids, plus per-level counts;
    /// `warn`-level signals participate in `--strict`, `notice`
    /// never does), labelling (grounded labels per declaring mem:
    /// accepted/defeated/undecided counts, the defeated and undecided
    /// lists with their attacker evidence, and the excluded cross-mem
    /// attack-edge count; an observation, never a strict violation).
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
    /// present violations, or when an always-on configuration axis
    /// reports findings. Always-on (no `--include` opt-in): the
    /// authoring-drift axis (`SCHEMA_AUTHORING_SOURCE_MISSING` /
    /// `SCHEMA_AUTHORING_SOURCE_DIVERGED`) and the configuration
    /// defects `SCHEMA_PIN_MISMATCH`, `SCHEMA_UNSTAMPED_SOURCE_ROT`
    /// and `MOUNT_UNBACKED` (a mount whose branch or folder does not
    /// exist, or holds no entity). Include-gated participation:
    /// `missing_required_outgoing`, `constraints`, `signals` (warn
    /// level), and with `integrity` the consistency findings
    /// `ORPHAN_STUB`, `DANGLING_LINK_TARGET_MISSING`,
    /// `DANGLING_LINK_NOT_RELATED` and
    /// `DANGLING_RELATION_TARGET_MISSING` and
    /// `CROSS_MEM_EDGE_UNGRANTED`. Stale entities, drifted
    /// anchors and `SCHEMA_GENERATIONS_BEHIND` stay advisory. The
    /// output is rendered first, then the non-zero exit fires; new
    /// Tier-2 codes opt in additively without breaking the flag's
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
        body_observations,
        config_entries,
        anchors_axis,
        ledger_axis,
        open_questions_axis,
        stale_derivations_axis,
        checks_axis,
        signals_axis,
        labelling_axis,
    } = match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(mut engine) => {
            let mut g = gather_mem_repo(&mut engine, args.limit, include);
            g.findings = gather_findings(&engine, include, args.target_schema.as_deref())?;
            g.body_observations =
                gather_body_observations(&engine, include, args.target_schema.as_deref())?;
            g
        }
        CliEngine::Filesystem(mut engine) => {
            let mut g = gather_filesystem(&mut engine, args.limit, include);
            g.findings = gather_findings(&engine, include, args.target_schema.as_deref())?;
            g.body_observations =
                gather_body_observations(&engine, include, args.target_schema.as_deref())?;
            g
        }
    };

    let mut result = json!({
        // The coverage rule (memstead_base::ops::coverage): the axes
        // this surface's verdict answers for, straight from the CLI's
        // registry row so output and declaration cannot diverge.
        // An opt-in axis rendered this pass was examined by it: `anchors`
        // moves into the examined set under `--include anchors`.
        "verdict_coverage": crate::coverage::HEALTH
            .axis_coverage()
            .expect("health is a verdict surface")
            .wire_line_promoting(if include.iter().any(|s| s == "anchors") {
                &["anchors"]
            } else {
                &[]
            }),
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
        // The consistency axis participates in `--strict` when asked
        // for: a dangling link or an orphan stub is a graph that says
        // something it cannot show, and a referee that ignored both
        // exited 0 on a workspace with ten of one and seven of the
        // other. Conformance findings keep their own reporting.
        if include.iter().any(|s| s == "integrity") {
            // Reads the family's own code list rather than a hand-written
            // one, so splitting the fused code could not silently drop two of
            // the three conditions out of the strict gate — the most likely
            // accidental outcome of that change (04/06, criterion 3).
            let dangling = findings
                .iter()
                .filter(|f| {
                    memstead_base::ops::DanglingLinkKind::ALL_CODES.contains(&f.code.as_str())
                })
                .count();
            if dangling > 0 {
                strict_violations.push(("dangling_links", dangling));
            }
            let orphan_stubs = findings.iter().filter(|f| f.code == "ORPHAN_STUB").count();
            if orphan_stubs > 0 {
                strict_violations.push(("orphan_stubs", orphan_stubs));
            }
            // An edge the write gate would refuse to create today is a
            // workspace whose policy file has stopped describing its graph.
            // Strict is opt-in and is exactly the gate an operator runs after
            // changing policy, so this is where the two are forced back into
            // agreement (04/07, criterion 3).
            let ungranted = findings
                .iter()
                .filter(|f| f.code == "CROSS_MEM_EDGE_UNGRANTED")
                .count();
            if ungranted > 0 {
                strict_violations.push(("ungranted_cross_mem_edges", ungranted));
            }
            // A sidecar the engine cannot read: every anchor surface over
            // that mem reports a condition instead of rows, and a strict run
            // that passed over it would be clean over the unmeasured.
            let unreadable = findings
                .iter()
                .filter(|f| f.code == "ANCHORS_SIDECAR_UNREADABLE")
                .count();
            if unreadable > 0 {
                strict_violations.push(("anchors_sidecar_unreadable", unreadable));
            }
        }
        obj.insert("findings".into(), serde_json::to_value(&findings)?);
        // Beside the findings, never among them (04/01, criterion 2). An
        // observation names content the type does not declare and says whether
        // it survives; it never marks the entity unconformant, and it is
        // deliberately absent from `strict_violations` above.
        obj.insert(
            "body_observations".into(),
            serde_json::to_value(&body_observations)?,
        );
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
        // The axis was asked for, so it is examined: a mem whose sidecar
        // could not be read is a strict violation here as well, or a
        // `--strict --include anchors` run would exit clean over a mem it
        // never measured.
        let unreadable = axis
            .as_object()
            .map(|mems| {
                mems.values()
                    .filter(|m| m.get("condition").is_some_and(|c| !c.is_null()))
                    .count()
            })
            .unwrap_or(0);
        if unreadable > 0
            && !strict_violations
                .iter()
                .any(|(k, _)| *k == "anchors_sidecar_unreadable")
        {
            strict_violations.push(("anchors_sidecar_unreadable", unreadable));
        }
        obj.insert("anchors".to_string(), axis.clone());
    }
    if let Some(axis) = &ledger_axis {
        obj.insert("ledger".to_string(), axis.clone());
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
    // `--include signals`: entities carrying above-`none` declared
    // signals, with per-level counts. A `warn`-level signal
    // participates in `--strict` like a warn-tier constraint finding;
    // a `notice` never does.
    if let Some(axis) = &signals_axis {
        if let Some(warn) = axis
            .get("counts")
            .and_then(|c| c.get("warn"))
            .and_then(|w| w.as_u64())
            && warn > 0
        {
            strict_violations.push(("signals", warn as usize));
        }
        obj.insert("signals".to_string(), axis.clone());
    }
    // `--include labelling`: grounded labels per declaring mem — a
    // reported observation with its evidence, never a strict
    // violation.
    if let Some(axis) = &labelling_axis {
        obj.insert("labelling".to_string(), axis.clone());
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
    // Per-file load failures — the same boot-honesty class: each
    // entry's message names the remedy (the merge-conflict refusal
    // names `memstead conflicts resolve`), and this hand-built
    // envelope must carry them like the MCP surfaces do or a
    // CLI-driven agent never finds the door (backlog-sweep plan 07).
    if !health.load_errors.is_empty() {
        obj.insert(
            "load_errors".into(),
            serde_json::to_value(&health.load_errors).unwrap_or_default(),
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
    // Configuration defects participate unconditionally too: a mount
    // whose pin disagrees with its mem's config, a pinned schema whose
    // sealed package has rotted, a mount that resolves to nothing.
    // None of them is about an entity; each is the workspace
    // describing itself wrongly, and `--strict` exited 0 on three pin
    // mismatches, two rotted schemas and two unbacked mounts until
    // 2026-08-23. Generations-behind pins stay advisory: the pin works.
    for (label, code) in [
        ("schema_pin_mismatch", "SCHEMA_PIN_MISMATCH"),
        ("schema_unstamped_source_rot", "SCHEMA_UNSTAMPED_SOURCE_ROT"),
        ("mount_unbacked", "MOUNT_UNBACKED"),
    ] {
        let n = health.warnings.iter().filter(|w| w.code() == code).count();
        if n > 0 {
            strict_violations.push((label, n));
        }
    }

    if ctx.json {
        print_json(&result)?;
        return strict_exit(args.strict, &strict_violations);
    }

    // Markdown rendering
    let mut lines = Vec::new();
    lines.push("# Graph health".to_string());
    lines.push(String::new());
    // The coverage rule: the axes the strict verdict answers for, in
    // the output itself (memstead_base::ops::coverage).
    if let Some(cov) = crate::coverage::HEALTH.axis_coverage() {
        lines.push(format!("**Verdict coverage:** {}", cov.wire_line()));
        lines.push(String::new());
    }
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
    // Conformance / integrity findings — the include was accepted and the
    // data gathered, so the human rendering must serve it: the JSON form
    // carried a populated `findings` array while this path printed only
    // the summary, and an operator diagnosing a mem by eye was told
    // nothing about content the engine was holding and reporting
    // (consistency-sweep 04/02's closing grade). An explicit zero is
    // rendered too, so "requested and clean" never reads as "not served".
    if let Some(v) = obj.get("findings").and_then(|v| v.as_array()) {
        lines.push(format!("## Conformance findings ({})", v.len()));
        if v.is_empty() {
            lines.push("- none".to_string());
        }
        for item in v {
            let mut line = format!(
                "- [{}] {} (axis {})",
                item["code"].as_str().unwrap_or("?"),
                item["id"].as_str().unwrap_or(""),
                item["axis"].as_str().unwrap_or("?"),
            );
            for key in ["field", "heading", "section"] {
                if let Some(val) = item["detail"][key].as_str() {
                    line.push_str(&format!(" — {key} `{val}`"));
                }
            }
            lines.push(line);
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("body_observations").and_then(|v| v.as_array())
        && !v.is_empty()
    {
        lines.push(format!("## Body observations ({})", v.len()));
        for item in v {
            let mut line = format!(
                "- [{}] {} — {}",
                item["code"].as_str().unwrap_or("?"),
                item["id"].as_str().unwrap_or(""),
                item["fate"].as_str().unwrap_or("?"),
            );
            for key in ["heading", "key"] {
                if let Some(val) = item["detail"][key].as_str() {
                    line.push_str(&format!(", {key} `{val}`"));
                }
            }
            lines.push(line);
        }
        lines.push(String::new());
    }
    // Same gap one include over: `--include constraints` filled the JSON
    // and the strict tally while this rendering said nothing.
    if let Some(v) = obj.get("constraints").and_then(|v| v.as_array()) {
        lines.push(format!("## Constraint violations ({})", v.len()));
        if v.is_empty() {
            lines.push("- none".to_string());
        }
        for item in v {
            let mut kinds: Vec<String> = item["violations"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x["kind"].as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if item["format_violations"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
            {
                kinds.push("section_format".to_string());
            }
            lines.push(format!(
                "- {} — {} ({})",
                item["id"].as_str().unwrap_or(""),
                item["title"].as_str().unwrap_or(""),
                kinds.join(", "),
            ));
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("schema_format_defects").and_then(|v| v.as_array()) {
        lines.push(format!("## Schema format defects ({})", v.len()));
        for item in v {
            lines.push(format!("- {}", item));
        }
        lines.push(String::new());
    }
    if let Some(v) = obj.get("dangling_links").and_then(|v| v.as_array()) {
        lines.push("## Dangling links".to_string());
        for item in v {
            // Name the condition and its repair. A reader used to get three
            // different problems in one shape and had to work out which by
            // noticing whether `section` was null (04/06, criterion 4).
            lines.push(format!(
                "- [{}] {} → {}{}",
                item["kind"].as_str().unwrap_or("?"),
                item["from"].as_str().unwrap_or(""),
                item["target_id"].as_str().unwrap_or(""),
                item["section"]
                    .as_str()
                    .map(|s| format!(" (in `{s}`)"))
                    .unwrap_or_default(),
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

    // The human-readable half of the ledger axis. Rendering it only in
    // `--json` would put the reconciliation out of reach of the operator who
    // runs `memstead health` by eye, which is the same class of gap this plan
    // exists to close (04/04, criterion 11).
    if let Some(axis) = ledger_axis.as_ref().and_then(|a| a.as_object()) {
        lines.push(format!("## Ledger vs files ({} folder mem(s))", axis.len()));
        if axis.is_empty() {
            lines.push(
                "- no folder mems: the check does not apply to git-branch storage, whose \
                 change set is a real two-tree diff"
                    .to_string(),
            );
        }
        for (mem, r) in axis {
            let ghosts = r["ledger_without_file"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0);
            let unlogged = r["file_without_ledger"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0);
            if ghosts == 0 && unlogged == 0 {
                lines.push(format!("- `{mem}`: ledger and files agree"));
                continue;
            }
            lines.push(format!(
                "- `{mem}`: {ghosts} recorded with no file, {unlogged} file(s) the ledger \
                 never mentions"
            ));
            for id in r["ledger_without_file"].as_array().into_iter().flatten() {
                lines.push(format!(
                    "  - recorded, no file: `{}`",
                    id.as_str().unwrap_or("")
                ));
            }
            for id in r["file_without_ledger"].as_array().into_iter().flatten() {
                lines.push(format!(
                    "  - file, never recorded: `{}`",
                    id.as_str().unwrap_or("")
                ));
            }
        }
        lines.push(String::new());
    }

    if let Some(axis) = anchors_axis.as_ref().and_then(|a| a.as_object()) {
        lines.push(format!("## Anchors ({} mems)", axis.len()));
        for (mem, counts) in axis {
            // The figure and its population in one rendering
            // (consistency-sweep 03/05, criteria 1 and 3). This is the
            // human-readable half of the health axis, and it used to print
            // four numbers and stop: no unobserved count, no population, no
            // statement of what was adjudicated. It reads its counts out of a
            // `serde_json::Value` by index, which is how it stayed invisible
            // to the figure check until that check learned the form.
            if let Some(c) = counts.get("condition").filter(|c| !c.is_null()) {
                lines.push(format!(
                    "- `{mem}`: ANCHORS_SIDECAR_UNREADABLE — {} — {}",
                    c["reason"].as_str().unwrap_or("reason not stated"),
                    counts["population"]
                        .as_str()
                        .unwrap_or("population not stated"),
                ));
                continue;
            }
            lines.push(format!(
                "- `{mem}`: resolves {}, drifted {}, recheck {}, unresolvable (artifact gone) \
                 {}, unobserved (not measured) {}, dangling (entity gone) {} — {}",
                counts["resolves"].as_u64().unwrap_or(0),
                counts["drifted"].as_u64().unwrap_or(0),
                counts["recheck"].as_u64().unwrap_or(0),
                counts["unresolvable"].as_u64().unwrap_or(0),
                counts["unobserved"].as_u64().unwrap_or(0),
                counts["dangling"].as_u64().unwrap_or(0),
                counts["population"]
                    .as_str()
                    .unwrap_or("population not stated"),
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
                // The bucket the axis inserts and counts into `total_open`,
                // which this list did not print, so a hole the axis had
                // measured never reached the reader (consistency-sweep 03/05).
                "anchors_unobserved",
                // Its sibling from 03/02, omitted for the same reason and
                // with the same effect: the axis counts it into `total_open`,
                // so a dangling row raised the total with nothing in the human
                // rendering saying why.
                "anchors_dangling",
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
            let conf = |key: &str| {
                c.get("conformance")
                    .and_then(|g| g.get(key))
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0)
            };
            let gate = |key: &str| {
                c.get("independence")
                    .and_then(|g| g.get(key))
                    .and_then(|e| e.get("count"))
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0)
            };
            lines.push(format!(
                "- `{mem}`: never_checked {}, checked_ok {}, check_failed {}, \
                 check_stale {}; conformance: never_checked {}, \
                 checked_ok {}, check_failed {}, check_stale {}; \
                 independence: self_checked {}, \
                 confirmed_independent {}, unconfirmable {}",
                count("never_checked"),
                count("checked_ok"),
                count("check_failed"),
                count("check_stale"),
                conf("never_checked"),
                conf("checked_ok"),
                conf("check_failed"),
                conf("check_stale"),
                gate("self_checked"),
                gate("confirmed_independent"),
                gate("unconfirmable"),
            ));
            // Foreign `x-` kinds by count, and the structured finding each
            // entity's newest record carries — the JSON axis has both;
            // the text surface says the same or it says less than it knows.
            if let Some(foreign) = c.get("foreign_kinds").and_then(|f| f.as_object())
                && !foreign.is_empty()
            {
                let listed: Vec<String> = foreign
                    .iter()
                    .map(|(k, n)| format!("{k} {}", n.as_u64().unwrap_or(0)))
                    .collect();
                lines.push(format!("  - foreign kinds: {}", listed.join(", ")));
            }
            if let Some(findings) = c.get("findings").and_then(|f| f.as_object()) {
                for (entity, f) in findings {
                    let code = f["finding"]["code"].as_str().unwrap_or("?");
                    let section = f["finding"]["section"]
                        .as_str()
                        .map(|s| format!(" [{s}]"))
                        .unwrap_or_default();
                    let message = f["finding"]["message"].as_str().unwrap_or("");
                    lines.push(format!(
                        "  - finding on `{entity}` ({} {}): {code}{section} — {message}",
                        f["kind"].as_str().unwrap_or("verification"),
                        f["verdict"].as_str().unwrap_or("?"),
                    ));
                }
            }
        }
        lines.push(String::new());
    }

    // Signals axis — every above-`none` signal with its evidence.
    if let Some(axis) = obj.get("signals") {
        lines.push(format!(
            "## Signals (notice {}, warn {})",
            axis["counts"]["notice"].as_u64().unwrap_or(0),
            axis["counts"]["warn"].as_u64().unwrap_or(0),
        ));
        for e in axis["entities"].as_array().into_iter().flatten() {
            for s in e["signals"].as_array().into_iter().flatten() {
                let contributors = s["contributors"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| c.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                lines.push(format!(
                    "- {} — {}: {} ({}) [{}]",
                    e["id"].as_str().unwrap_or(""),
                    s["name"].as_str().unwrap_or(""),
                    s["value"].as_u64().unwrap_or(0),
                    s["level"].as_str().unwrap_or(""),
                    contributors,
                ));
            }
        }
        lines.push(String::new());
    }

    // Labelling axis — grounded labels with their evidence.
    if let Some(axis) = obj.get("labelling").and_then(|a| a.as_object()) {
        lines.push(format!("## Labelling ({} mems)", axis.len()));
        for (mem, m) in axis {
            let c = &m["counts"];
            lines.push(format!(
                "- `{mem}`: accepted {}, defeated {}, undecided {}; cross-mem attack edges excluded {}",
                c["accepted"].as_u64().unwrap_or(0),
                c["defeated"].as_u64().unwrap_or(0),
                c["undecided"].as_u64().unwrap_or(0),
                m["cross_mem_edges_excluded"].as_u64().unwrap_or(0),
            ));
            for d in m["defeated"].as_array().into_iter().flatten() {
                let by = d["defeated_by"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                lines.push(format!(
                    "  - defeated: {} (by {by})",
                    d["id"].as_str().unwrap_or("")
                ));
            }
            for u in m["undecided"].as_array().into_iter().flatten() {
                let by = u["undecided_by"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                lines.push(format!(
                    "  - undecided: {} (open attackers {by})",
                    u["id"].as_str().unwrap_or("")
                ));
            }
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

    // Per-file load failures — ungated like the quarantine roster;
    // each message names its remedy, so the markdown must show it.
    if let Some(arr) = obj.get("load_errors").and_then(|v| v.as_array()) {
        lines.push(format!("## Load errors ({})", arr.len()));
        for e in arr {
            lines.push(format!(
                "- `{}` — {}",
                e.get("file").and_then(|x| x.as_str()).unwrap_or(""),
                e.get("error").and_then(|x| x.as_str()).unwrap_or(""),
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
                // Reason breakdown where recorded — a code without
                // recorded reasons renders exactly as before.
                if let Some(reasons) = f["by_reason"][code.as_str()]
                    .as_object()
                    .filter(|m| !m.is_empty())
                {
                    let mut rs: Vec<(&String, u64)> = reasons
                        .iter()
                        .map(|(k, v)| (k, v.as_u64().unwrap_or(0)))
                        .collect();
                    rs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
                    for (reason, count) in rs {
                        lines.push(format!("    - {reason}: {count}"));
                    }
                }
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
    /// Body observations (consistency-sweep 04/01) — what an entity's stored
    /// body carries that its type does not declare. Beside the findings, never
    /// among them: an observation is not a violation.
    body_observations: Vec<memstead_base::ops::integrity::BodyObservation>,
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
    /// per-mem anchor-verification counts (with `unresolvable` meaning the
    /// artifact is gone and `unobserved` meaning the pass could not measure
    /// it) plus the population they cover, from the shared
    /// `health_anchors_axis` helper (same axis MCP renders). `None`
    /// otherwise — absence of the key means "not requested".
    anchors_axis: Option<serde_json::Value>,
    /// `--include ledger`: a folder mem's ledger set against its file set.
    ledger_axis: Option<serde_json::Value>,
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
    /// `--include signals` — the shared `health_signals_axis`
    /// payload (entities above `none` plus per-level counts).
    signals_axis: Option<serde_json::Value>,
    /// `--include labelling` — the shared `health_labelling_axis`
    /// payload (per declaring mem: label counts, defeated/undecided
    /// lists with attacker evidence, excluded cross-mem edges).
    labelling_axis: Option<serde_json::Value>,
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

/// Body observations for every mem, when the caller asked for the conformance
/// or integrity axis (consistency-sweep 04/01).
///
/// Gathered beside the findings and rendered beside them, never among them:
/// an observation is not a violation and must never reach `strict_violations`,
/// because absorbing an undeclared heading is the catch-all working as
/// designed. What the reader gets is the distinction the axis could not make
/// before: content that was absorbed and survives, against content the next
/// write does not keep.
fn gather_body_observations(
    engine: &memstead_base::Engine,
    include: &[String],
    target_schema: Option<&str>,
) -> anyhow::Result<Vec<memstead_base::ops::integrity::BodyObservation>> {
    if !include
        .iter()
        .any(|s| s == "conformance" || s == "integrity")
    {
        return Ok(Vec::new());
    }
    let target = match target_schema {
        None => None,
        Some(raw) => Some(
            raw.parse::<memstead_schema::SchemaRef>()
                .map_err(|reason| anyhow::anyhow!("invalid --target-schema {raw:?}: {reason}"))?,
        ),
    };
    let mut mems: Vec<String> = engine.schemas().keys().cloned().collect();
    mems.sort();
    let mut out = Vec::new();
    for v in &mems {
        out.extend(
            engine
                .body_observations(v, target.as_ref())
                .map_err(crate::CliError::from_engine_op)?,
        );
    }
    Ok(out)
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
    fill_signals_axis(engine, include, &mut g);
    fill_labelling_axis(engine, include, &mut g);
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
    fill_signals_axis(engine, include, &mut g);
    fill_labelling_axis(engine, include, &mut g);
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

/// Engine-aware step for `--include anchors` — the per-mem anchor-verification
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
        g.open_questions_axis = Some(memstead_base::ops::health::health_open_questions_axis(
            engine, None,
        ));
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
        g.stale_derivations_axis = Some(memstead_base::ops::health::health_stale_derivations_axis(
            engine, None,
        ));
    }
}

fn fill_checks_axis(engine: &memstead_base::Engine, include: &[String], g: &mut GatheredHealth) {
    if include.iter().any(|s| s == "checks") {
        g.checks_axis = Some(memstead_base::ops::health::health_checks_axis(engine, None));
    }
}

fn fill_signals_axis(engine: &memstead_base::Engine, include: &[String], g: &mut GatheredHealth) {
    if include.iter().any(|s| s == "signals") {
        g.signals_axis = Some(engine.health_signals_axis(None));
    }
}

fn fill_labelling_axis(engine: &memstead_base::Engine, include: &[String], g: &mut GatheredHealth) {
    if include.iter().any(|s| s == "labelling") {
        g.labelling_axis = Some(engine.health_labelling_axis(None));
    }
}

fn fill_anchors_axis(engine: &memstead_base::Engine, include: &[String], g: &mut GatheredHealth) {
    if include.iter().any(|s| s == "anchors") {
        g.anchors_axis = Some(memstead_base::ops::health::health_anchors_axis(engine));
    }
    // Folder mems only; a git-branch mem is absent rather than clean
    // (04/04, criterion 4).
    if include.iter().any(|s| s == "ledger") {
        g.ledger_axis = serde_json::to_value(engine.ledger_reconciliation()).ok();
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
///
/// Ten parameters is deliberate: five of them are the engine-shaped callbacks
/// that keep this function engine-agnostic. Bundling them into a struct would
/// move the same arity behind a type that exists for one call site.
#[allow(clippy::too_many_arguments)]
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
        ledger_axis: None,
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
        body_observations: Vec::new(),
        config_entries: None,
        anchors_axis: None,
        open_questions_axis: None,
        stale_derivations_axis: None,
        checks_axis: None,
        signals_axis: None,
        labelling_axis: None,
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
