//! `memstead health` — the workspace health report, composed by the
//! engine's shared `compose_health` (the same builder behind the MCP
//! `memstead_health` tool) so the `--json` bytes equal the tool's
//! `structured_content` for every include key and under a `--mem` filter
//! (backlog-engine plan A7). This file owns only the CLI's concerns: the
//! argument shape, the `--strict` exit policy read off the composed report,
//! and the markdown rendering.

use clap::Parser;
use memstead_base::ops::health_compose::{
    ComposeHealthError, HealthArgs, HealthConfig, compose_health,
};
use serde_json::Value;

use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

/// Graph health summary.
///
/// Default: counts only. Pass `--include` to drill into details.
#[derive(Parser, Debug)]
pub struct Args {
    /// Scope the report to one writable mem (the engine still loads every
    /// mem: dangling-link adjudication and the community partition are
    /// only truthful over the whole store).
    #[arg(long)]
    pub mem: Option<String>,

    /// Opt heavy content into the response: orphans, stubs,
    /// most_connected, missing_fields, stale (two clocks, one per
    /// entity: an entity with an adjudicated hash-bearing anchor reads
    /// by its anchors, `drifted` and `recheck` listed as their own
    /// condition and `resolves` kept off the list, the row naming the
    /// anchor clock and its state, with `anchor_fresh` holding the
    /// entities the day threshold would have listed; every other entity
    /// reads by the type's `staleness_threshold_days` as before, its row
    /// unchanged), dangling_links, tags,
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
    /// engine-defined vocabularies only), vital_signs (per-mem model-truth signals: last-resort type share per community, unclaimed and contested source files, zero-outgoing entities folded into their subject, empty declared sections; counts and capped lists, never a verdict), open_questions (per-mem
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
    /// `UNRESOLVED_STUB`, `DANGLING_LINK_TARGET_MISSING`,
    /// `DANGLING_LINK_NOT_RELATED` and
    /// `DANGLING_RELATION_TARGET_MISSING` and
    /// `CROSS_MEM_EDGE_UNGRANTED` (no grant declared for the pair
    /// while the target is mounted; an edge into a mem that is not
    /// mounted is the dangling finding, once, never a grant finding,
    /// whatever the grant table still names). Stale entities, drifted
    /// anchors and `SCHEMA_GENERATIONS_BEHIND` stay advisory. The
    /// output is rendered first, then the non-zero exit fires; new
    /// Tier-2 codes opt in additively without breaking the flag's
    /// semantics.
    #[arg(long)]
    pub strict: bool,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut cli_engine = ctx.cli_engine()?;
    let engine = cli_engine.base_mut();
    // Mirror the MCP handler: the full lazy-mount load, then the drift
    // pass whose warnings ride in the report.
    engine.ensure_mems_loaded(None);
    let drift_warnings = engine.reload_if_stale(args.mem.as_deref());
    let _ = engine.take_mem_changed_notices();

    let (mutations, plugin) =
        memstead_base::ops::health::config_projection_from_settings(engine.settings());
    let config = HealthConfig { mutations, plugin };
    let health_args = HealthArgs {
        mem: args.mem.as_deref(),
        include: &args.include,
        limit: Some(args.limit),
        target_schema: args.target_schema.as_deref(),
        include_config: false,
    };

    let result = match compose_health(engine, &health_args, drift_warnings, &config) {
        Ok(v) => v,
        Err(ComposeHealthError::MemQuarantined(name)) => {
            return Err(crate::CliError::from_engine_op(engine.unknown_mem_error(&name)).into());
        }
        Err(ComposeHealthError::UnknownMem {
            name,
            writable_mems,
        }) => {
            return Err(crate::CliError {
                code: "UNKNOWN_MEM",
                kind: ExitKind::NotFound,
                message: format!(
                    "unknown mem: \"{name}\". Writable mems: [{}]",
                    writable_mems.join(", ")
                ),
                details: Some(serde_json::json!({
                    "name": name,
                    "writable_mems": writable_mems,
                })),
            }
            .into());
        }
        Err(ComposeHealthError::InvalidTargetSchema { raw, reason }) => {
            return Err(crate::CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!("invalid target_schema {raw:?}: {reason}"),
            )
            .into());
        }
        Err(ComposeHealthError::Engine(e)) => {
            return Err(crate::CliError::from_engine_op(e).into());
        }
    };

    let strict_violations = strict_violations(&result, &args.include);

    if ctx.json {
        print_json(&result)?;
        return strict_exit(args.strict, &strict_violations);
    }

    print_markdown(&render_markdown(&result, args.mem.as_deref()));
    strict_exit(args.strict, &strict_violations)
}

/// The Tier-2 violations `--strict` refuses on, read off the composed
/// report. Every entry is a section the caller opted into with
/// `--include` (or a configuration defect the engine always reports), so
/// `--strict` without any Tier-2 include stays a no-op.
fn strict_violations(v: &Value, include: &[String]) -> Vec<(&'static str, usize)> {
    let has = |key: &str| include.iter().any(|s| s == key);
    let arr_len = |key: &str| v.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    let mut out: Vec<(&'static str, usize)> = Vec::new();
    fn push(out: &mut Vec<(&'static str, usize)>, label: &'static str, n: usize) {
        if n > 0 {
            out.push((label, n));
        }
    }

    if has("missing_required_outgoing") {
        push(
            &mut out,
            "missing_required_outgoing",
            arr_len("missing_required_outgoing"),
        );
    }
    if has("constraints") {
        push(&mut out, "constraints", arr_len("constraints"));
        push(
            &mut out,
            "schema_format_defects",
            arr_len("schema_format_defects"),
        );
    }
    if has("integrity") {
        let findings = v.get("findings").and_then(Value::as_array);
        let count_code = |pred: &dyn Fn(&str) -> bool| {
            findings.map_or(0, |f| {
                f.iter()
                    .filter(|x| x["code"].as_str().is_some_and(pred))
                    .count()
            })
        };
        push(
            &mut out,
            "dangling_links",
            count_code(&|c| memstead_base::ops::DanglingLinkKind::ALL_CODES.contains(&c)),
        );
        push(
            &mut out,
            "unresolved_stubs",
            count_code(&|c| c == "UNRESOLVED_STUB"),
        );
        push(
            &mut out,
            "ungranted_cross_mem_edges",
            count_code(&|c| c == "CROSS_MEM_EDGE_UNGRANTED"),
        );
        push(
            &mut out,
            "anchors_sidecar_unreadable",
            count_code(&|c| c == "ANCHORS_SIDECAR_UNREADABLE"),
        );
    }
    if let Some(warn) = v["signals"]["counts"]["warn"].as_u64() {
        push(&mut out, "signals", warn as usize);
    }
    if let Some(mems) = v.get("anchors").and_then(Value::as_object)
        && !out.iter().any(|(k, _)| *k == "anchors_sidecar_unreadable")
    {
        let unreadable = mems
            .values()
            .filter(|m| m.get("condition").is_some_and(|c| !c.is_null()))
            .count();
        push(&mut out, "anchors_sidecar_unreadable", unreadable);
    }

    let warnings = v.get("warnings").and_then(Value::as_array);
    let count_warning = |pred: &dyn Fn(&str) -> bool| {
        warnings.map_or(0, |w| {
            w.iter()
                .filter(|x| x["code"].as_str().is_some_and(pred))
                .count()
        })
    };
    push(
        &mut out,
        "schema_authoring_drift",
        count_warning(&|c| {
            matches!(
                c,
                "SCHEMA_AUTHORING_SOURCE_MISSING" | "SCHEMA_AUTHORING_SOURCE_DIVERGED"
            )
        }),
    );
    for (label, code) in [
        ("schema_pin_mismatch", "SCHEMA_PIN_MISMATCH"),
        ("schema_unstamped_source_rot", "SCHEMA_UNSTAMPED_SOURCE_ROT"),
        ("mount_unbacked", "MOUNT_UNBACKED"),
    ] {
        push(&mut out, label, count_warning(&|c| c == code));
    }
    out
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn n(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

fn strs(v: &Value) -> Vec<&str> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// Descending-count, then name: the order the markdown lists keyed maps in.
fn counts_desc(map: &serde_json::Map<String, Value>) -> Vec<(&String, u64)> {
    let mut entries: Vec<(&String, u64)> = map
        .iter()
        .map(|(k, v)| (k, v.as_u64().unwrap_or(0)))
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    entries
}

/// The CLI's markdown rendering of the composed report — every section
/// reads the same keys the JSON carries.
fn render_markdown(v: &Value, mem: Option<&str>) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("# Graph health".to_string());
    lines.push(String::new());
    if let Some(cov) = crate::coverage::HEALTH.axis_coverage() {
        lines.push(format!("**Verdict coverage:** {}", cov.wire_line()));
        lines.push(String::new());
    }
    if let Some(m) = mem {
        lines.push(format!("**Mem filter:** `{m}`"));
        lines.push(String::new());
    }
    let summary = &v["summary"];
    lines.push(format!("- Entities: {}", n(summary, "total_entities")));
    match summary["orphans_by_schema"].as_object() {
        Some(by) if by.len() > 1 => {
            let listed: Vec<String> = by
                .iter()
                .map(|(schema, count)| {
                    format!(
                        "{}: {}",
                        if schema.is_empty() {
                            "(unpinned)"
                        } else {
                            schema
                        },
                        count.as_u64().unwrap_or(0)
                    )
                })
                .collect();
            lines.push(format!(
                "- Orphans: {} ({})",
                n(summary, "total_orphans"),
                listed.join(", ")
            ));
        }
        _ => lines.push(format!("- Orphans: {}", n(summary, "total_orphans"))),
    }
    lines.push(format!("- Stubs: {}", n(summary, "total_stubs")));
    lines.push(format!("- Stale: {}", n(summary, "total_stale")));
    lines.push(format!(
        "- Missing fields: {}",
        n(summary, "total_missing_fields")
    ));
    lines.push(format!(
        "- Communities: {}",
        n(summary, "total_communities")
    ));
    lines.push(String::new());

    if let Some(items) = v.get("orphans").and_then(Value::as_array) {
        lines.push("## Orphans".to_string());
        for item in items {
            lines.push(format!("- {} — {}", s(item, "id"), s(item, "title")));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("stubs").and_then(Value::as_array) {
        lines.push("## Stubs".to_string());
        for item in items {
            lines.push(format!("- {}", s(item, "id")));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("most_connected").and_then(Value::as_array) {
        lines.push("## Most connected".to_string());
        lines.push("(ranked by typed dependency degree; total keeps mention edges)".to_string());
        for item in items {
            lines.push(format!(
                "- {} — {} (typed {}, total {}, in {}, out {})",
                s(item, "id"),
                s(item, "title"),
                n(item, "typed_total"),
                n(item, "total"),
                n(item, "incoming"),
                n(item, "outgoing"),
            ));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("missing_fields").and_then(Value::as_array) {
        lines.push("## Missing fields".to_string());
        for item in items {
            let labels: Vec<String> = match item["issues"].as_array() {
                Some(issues) if !issues.is_empty() => issues
                    .iter()
                    .map(|i| {
                        format!(
                            "{} ({})",
                            s(i, "field"),
                            i["code"].as_str().unwrap_or("MISSING")
                        )
                    })
                    .collect(),
                _ => strs(&item["missing"])
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            };
            lines.push(format!(
                "- {} — {} (issues: {})",
                s(item, "id"),
                s(item, "title"),
                labels.join(", ")
            ));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("stale").and_then(Value::as_array) {
        lines.push("## Stale entities".to_string());
        for item in items {
            lines.push(stale_line(item));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("anchor_fresh").and_then(Value::as_array) {
        lines.push("## Fresh by anchor clock".to_string());
        for item in items {
            lines.push(stale_line(item));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("missing_required_outgoing").and_then(Value::as_array) {
        lines.push("## Missing required outgoing".to_string());
        for item in items {
            let blocks: Vec<String> = item["missing"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|b| {
                            format!(
                                "[{}] {}",
                                strs(&b["relationships"]).join(", "),
                                s(b, "cardinality")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            lines.push(format!(
                "- {} — {} (missing: {})",
                s(item, "id"),
                s(item, "title"),
                blocks.join("; ")
            ));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("findings").and_then(Value::as_array) {
        lines.push(format!("## Conformance findings ({})", items.len()));
        if items.is_empty() {
            lines.push("- none".to_string());
        }
        for item in items {
            let mut line = format!(
                "- [{}] {} (axis {})",
                item["code"].as_str().unwrap_or("?"),
                s(item, "id"),
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
    if let Some(items) = v.get("body_observations").and_then(Value::as_array)
        && !items.is_empty()
    {
        lines.push(format!("## Body observations ({})", items.len()));
        for item in items {
            let mut line = format!(
                "- [{}] {} — {}",
                item["code"].as_str().unwrap_or("?"),
                s(item, "id"),
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
    if let Some(items) = v.get("constraints").and_then(Value::as_array) {
        lines.push(format!("## Constraint violations ({})", items.len()));
        if items.is_empty() {
            lines.push("- none".to_string());
        }
        for item in items {
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
                s(item, "id"),
                s(item, "title"),
                kinds.join(", "),
            ));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("schema_format_defects").and_then(Value::as_array) {
        lines.push(format!("## Schema format defects ({})", items.len()));
        for item in items {
            lines.push(format!("- {item}"));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("dangling_links").and_then(Value::as_array) {
        lines.push("## Dangling links".to_string());
        for item in items {
            lines.push(format!(
                "- [{}] {} → {}{}",
                item["kind"].as_str().unwrap_or("?"),
                s(item, "from"),
                s(item, "target_id"),
                item["section"]
                    .as_str()
                    .map(|sec| format!(" (in `{sec}`)"))
                    .unwrap_or_default(),
            ));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("tag_distribution").and_then(Value::as_array) {
        lines.push("## Tags".to_string());
        for item in items {
            lines.push(format!("- {} ({})", s(item, "tag"), n(item, "count")));
        }
        lines.push(String::new());
    }
    if let Some(items) = v.get("warnings").and_then(Value::as_array) {
        lines.push("## Warnings".to_string());
        for w in items {
            lines.push(format!("- {} — {}", s(w, "code"), s(w, "message")));
        }
        lines.push(String::new());
    }
    if let Some(u) = v.get("untagged_entities") {
        lines.push("## Untagged".to_string());
        lines.push(format!("- Total: {}", n(u, "total")));
        if let Some(by_type) = u["by_entity_type"].as_object() {
            for (kind, count) in counts_desc(by_type) {
                lines.push(format!("  - {kind}: {count}"));
            }
        }
        lines.push(String::new());
    }

    if let Some(axis) = v.get("ledger").and_then(Value::as_object) {
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

    if let Some(axis) = v.get("anchors").and_then(Value::as_object) {
        lines.push(format!("## Anchors ({} mems)", axis.len()));
        for (mem, counts) in axis {
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
                n(counts, "resolves"),
                n(counts, "drifted"),
                n(counts, "recheck"),
                n(counts, "unresolvable"),
                n(counts, "unobserved"),
                n(counts, "dangling"),
                counts["population"]
                    .as_str()
                    .unwrap_or("population not stated"),
            ));
        }
        lines.push(String::new());
    }

    if let Some(axis) = v.get("vital_signs").and_then(Value::as_object) {
        let mems: Vec<(&String, &Value)> = axis.iter().filter(|(k, _)| *k != "_item_cap").collect();
        lines.push(format!("## Vital signs ({} mems)", mems.len()));
        for (mem, sig) in mems {
            let count = |k: &str| sig[k]["count"].as_u64().unwrap_or(0);
            let share = match sig["type_share_by_community"]["status"].as_str() {
                Some("declared") => format!(
                    "last-resort type `{}` over {} community(ies)",
                    sig["type_share_by_community"]["last_resort_type"]
                        .as_str()
                        .unwrap_or("?"),
                    count("type_share_by_community")
                ),
                _ => "last-resort type not declared".to_string(),
            };
            let unclaimed = match sig["unclaimed_source_files"]["status"].as_str() {
                Some("enumerated") => {
                    format!(
                        "{} unclaimed source file(s)",
                        count("unclaimed_source_files")
                    )
                }
                _ => "no bound source".to_string(),
            };
            lines.push(format!(
                "- `{mem}`: {share}; {unclaimed}; {} contested unowned file(s); {} zero-outgoing \
                 entity(ies) in {} community(ies); {} empty declared section(s)",
                count("contested_unowned_files"),
                sig["zero_outgoing_entities"]["entities"]
                    .as_u64()
                    .unwrap_or(0),
                count("zero_outgoing_entities"),
                count("empty_declared_sections"),
            ));
        }
        lines.push(String::new());
    }

    if let Some(axis) = v.get("open_questions").and_then(Value::as_object) {
        let cap = axis
            .get("_item_cap")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        lines.push(format!("## Open questions (item cap {cap} per kind)"));
        for (mem, entry) in axis.iter().filter(|(k, _)| *k != "_item_cap") {
            lines.push(format!("- `{mem}`: {} open", n(entry, "total_open")));
            for kind in [
                "stubs",
                "anchors_recheck",
                "anchors_unresolvable",
                "anchors_unobserved",
                "anchors_dangling",
                "unsatisfied_constraints",
                "dangling_links",
                "resolution_missing",
                "resolution_unchecked",
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
            for p in entry["process"].as_array().into_iter().flatten() {
                if p["resolvable"] == Value::Bool(true) {
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
        lines.push(String::new());
    }

    if let Some(axis) = v.get("checks").and_then(Value::as_object) {
        lines.push(format!("## Checks ({} mems)", axis.len()));
        for (mem, c) in axis {
            let conf = |key: &str| c["conformance"][key].as_u64().unwrap_or(0);
            let gate = |key: &str| c["independence"][key]["count"].as_u64().unwrap_or(0);
            lines.push(format!(
                "- `{mem}`: never_checked {}, checked_ok {}, check_failed {}, \
                 check_stale {}; conformance: never_checked {}, \
                 checked_ok {}, check_failed {}, check_stale {}; \
                 independence: self_checked {}, \
                 confirmed_independent {}, unconfirmable {}",
                n(c, "never_checked"),
                n(c, "checked_ok"),
                n(c, "check_failed"),
                n(c, "check_stale"),
                conf("never_checked"),
                conf("checked_ok"),
                conf("check_failed"),
                conf("check_stale"),
                gate("self_checked"),
                gate("confirmed_independent"),
                gate("unconfirmable"),
            ));
            if let Some(foreign) = c.get("foreign_kinds").and_then(Value::as_object)
                && !foreign.is_empty()
            {
                let listed: Vec<String> = foreign
                    .iter()
                    .map(|(k, count)| format!("{k} {}", count.as_u64().unwrap_or(0)))
                    .collect();
                lines.push(format!("  - foreign kinds: {}", listed.join(", ")));
            }
            if let Some(findings) = c.get("findings").and_then(Value::as_object) {
                for (entity, f) in findings {
                    let code = f["finding"]["code"].as_str().unwrap_or("?");
                    let section = f["finding"]["section"]
                        .as_str()
                        .map(|sec| format!(" [{sec}]"))
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

    if let Some(axis) = v.get("signals") {
        lines.push(format!(
            "## Signals (notice {}, warn {})",
            axis["counts"]["notice"].as_u64().unwrap_or(0),
            axis["counts"]["warn"].as_u64().unwrap_or(0),
        ));
        for e in axis["entities"].as_array().into_iter().flatten() {
            for sig in e["signals"].as_array().into_iter().flatten() {
                lines.push(format!(
                    "- {} — {}: {} ({}) [{}]",
                    s(e, "id"),
                    s(sig, "name"),
                    n(sig, "value"),
                    s(sig, "level"),
                    strs(&sig["contributors"]).join(", "),
                ));
            }
        }
        lines.push(String::new());
    }

    if let Some(axis) = v.get("labelling").and_then(Value::as_object) {
        lines.push(format!("## Labelling ({} mems)", axis.len()));
        for (mem, m) in axis {
            let c = &m["counts"];
            lines.push(format!(
                "- `{mem}`: accepted {}, defeated {}, undecided {}; cross-mem attack edges excluded {}",
                n(c, "accepted"),
                n(c, "defeated"),
                n(c, "undecided"),
                n(m, "cross_mem_edges_excluded"),
            ));
            for d in m["defeated"].as_array().into_iter().flatten() {
                lines.push(format!(
                    "  - defeated: {} (by {})",
                    s(d, "id"),
                    strs(&d["defeated_by"]).join(", ")
                ));
            }
            for u in m["undecided"].as_array().into_iter().flatten() {
                lines.push(format!(
                    "  - undecided: {} (open attackers {})",
                    s(u, "id"),
                    strs(&u["undecided_by"]).join(", ")
                ));
            }
        }
        lines.push(String::new());
    }

    if let Some(axis) = v.get("stale_derivations").and_then(Value::as_object) {
        let total: usize = axis
            .values()
            .filter_map(|a| a.as_array().map(Vec::len))
            .sum();
        lines.push(format!("## Stale derivations ({total} findings)"));
        for (mem, findings) in axis {
            for f in findings.as_array().into_iter().flatten() {
                lines.push(format!(
                    "- `{mem}`: {} -[{}]-> {} ({})",
                    s(f, "source"),
                    s(f, "rel_type"),
                    s(f, "target"),
                    s(f, "state"),
                ));
            }
        }
        lines.push(String::new());
    }

    if let Some(items) = v.get("quarantined").and_then(Value::as_array) {
        lines.push(format!("## Quarantined mems ({})", items.len()));
        for q in items {
            lines.push(format!(
                "- `{}` [{}] {}",
                s(q, "mem"),
                s(q, "reason_code"),
                s(q, "reason_message"),
            ));
        }
        lines.push(String::new());
    }

    if let Some(items) = v.get("load_errors").and_then(Value::as_array) {
        lines.push(format!("## Load errors ({})", items.len()));
        for e in items {
            lines.push(format!("- `{}` — {}", s(e, "file"), s(e, "error")));
        }
        lines.push(String::new());
    }

    if let Some(f) = v.get("friction") {
        lines.push(format!(
            "## Friction ({} refusals recorded, {} in the last 24h)",
            n(f, "total"),
            f["recent_24h"]["total"].as_u64().unwrap_or(0),
        ));
        if let Some(by_code) = f["by_code"].as_object().filter(|m| !m.is_empty()) {
            lines.push("- by code:".to_string());
            for (code, count) in counts_desc(by_code) {
                lines.push(format!("  - {code}: {count}"));
                if let Some(reasons) = f["by_reason"][code.as_str()]
                    .as_object()
                    .filter(|m| !m.is_empty())
                {
                    for (reason, count) in counts_desc(reasons) {
                        lines.push(format!("    - {reason}: {count}"));
                    }
                }
            }
        }
        if let Some(by_verb) = f["by_verb"].as_object().filter(|m| !m.is_empty()) {
            lines.push("- by verb:".to_string());
            for (verb, count) in counts_desc(by_verb) {
                lines.push(format!("  - {verb}: {count}"));
            }
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

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

/// One stale-axis line: the day-threshold form unchanged, the anchor-clock
/// form naming the state that produced the row.
fn stale_line(item: &Value) -> String {
    let base = format!(
        "- {} — {} ({} days)",
        s(item, "id"),
        s(item, "title"),
        n(item, "days_since_modified")
    );
    match item.get("anchor_state").and_then(Value::as_str) {
        Some(state) => format!("{base} (anchor clock: {state})"),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use memstead_base::ops::health::HEALTH_INCLUDE_KEYS;

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

    #[test]
    fn strict_reads_the_tier_two_sections_off_the_report() {
        let v = serde_json::json!({
            "findings": [
                {"code": "UNRESOLVED_STUB"},
                {"code": "DANGLING_LINK_TARGET_MISSING"},
                {"code": "CROSS_MEM_EDGE_UNGRANTED"},
            ],
            "constraints": [{"id": "a"}],
            "signals": {"counts": {"warn": 2}},
            "warnings": [{"code": "MOUNT_UNBACKED"}],
        });
        let include = vec!["integrity".to_string(), "constraints".to_string()];
        let got = strict_violations(&v, &include);
        assert_eq!(
            got,
            vec![
                ("constraints", 1),
                ("dangling_links", 1),
                ("unresolved_stubs", 1),
                ("ungranted_cross_mem_edges", 1),
                ("signals", 2),
                ("mount_unbacked", 1),
            ]
        );
        // Sections not opted into do not count.
        let got = strict_violations(&v, &[]);
        assert_eq!(got, vec![("signals", 2), ("mount_unbacked", 1)]);
    }
}
