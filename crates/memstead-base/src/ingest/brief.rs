//! Run-brief rendering — the engine-side assembly of the Markdown brief an
//! ingest agent consumes as its prompt.
//!
//! The brief is a **rendered string** by deliberate design: an agent reads
//! it as a prompt, so a rendered Markdown contract (matching the plugin's
//! `inject.mjs` stdout) is the natural boundary, and parity between clients
//! is checked on the rendered bytes. Each block function returns a string
//! ending in a blank line (or the empty string), and the full brief is the
//! truthy blocks concatenated.
//!
//! All three modes are assembled here: [`assemble_discovery_brief`] (with the
//! header blocks [`render_situation`], [`render_intent`],
//! [`render_goal_and_avoid`], [`render_operative_data`]),
//! [`assemble_refinement_brief`], and [`assemble_one_shot_brief`] — plus the
//! changed-slice preface ([`render_changed_slice`], rendered from a
//! [`SourceCursor`]).

use super::guidance::ResolvedGuidance;
use super::resolve::{ResolvedIngest, ResolvedSource};
use super::slice::Slice;
use crate::pipeline::{IngestMode, MediumType, PatternMode};

/// Per-class cap on the rendered changed slice — mirrors the plugin's
/// `SLICE_CAP`. Beyond it a `…and N more` line stands in.
const SLICE_CAP: usize = 25;

/// The schema every `ingest/<name>` process mem pins — mirrors the plugin's
/// `PROCESS_MEM_SCHEMA`.
pub const PROCESS_MEM_SCHEMA: &str = "ingest@0.1.0";

/// The paired-process-mem state the brief blocks read — the engine-side of
/// the plugin's `processMem` object. Whether a process mem is present /
/// skipped (one-shot) / failed-to-create is decided by the orchestration
/// glue; the blocks render from this resolved view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessMemInfo {
    /// A paired process mem exists and is usable.
    pub present: bool,
    /// No process mem is paired (one-shot ingests are ephemeral by design).
    pub skipped: bool,
    /// Auto-creation was attempted and failed; the notice explains why.
    pub notice: Option<String>,
    /// The process mem's leaf name (the ingest name) — its searchable id.
    pub leaf_name: String,
    /// The process mem's org-path label, `ingest/<name>`.
    pub mem_label: String,
}

/// The mode string the situation block prints (`discovery` / `refinement` /
/// `one-shot`) — the same tokens the plugin uses.
fn mode_label(mode: IngestMode) -> &'static str {
    match mode {
        IngestMode::Discovery => "discovery",
        IngestMode::Refinement => "refinement",
        IngestMode::OneShot => "one-shot",
    }
}

/// The medium-type label a source line prints — the lowercase medium `type`.
fn medium_type_label(t: MediumType) -> &'static str {
    match t {
        MediumType::Codebase => "codebase",
        MediumType::Filesystem => "filesystem",
        MediumType::Graph => "graph",
        MediumType::Git => "git",
        MediumType::Web => "web",
    }
}

/// Render the `## Goal` and `## Failure modes to avoid` blocks from resolved
/// guidance, matching the plugin's `goalAndAvoidBlock`. Each present field
/// contributes a header, a blank line, its trimmed prose, and a trailing
/// blank line; the block ends in a blank line. With neither field present
/// this yields `"\n"` (the plugin's `lines.join('\n') + '\n'` for the
/// no-pass-through case).
///
/// Pass-through-only guidance (a schema declaring `granularity`/`stack`/… but
/// no goal/avoid) is not yet rendered here — that fallback
/// (`renderResolvedGuidance`) lands with the pass-through modelling.
pub fn render_goal_and_avoid(guidance: &ResolvedGuidance) -> String {
    let mut lines: Vec<String> = Vec::new();

    if let Some(goal) = guidance
        .goal
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        lines.push("## Goal".to_string());
        lines.push(String::new());
        lines.push(goal.to_string());
        lines.push(String::new());
    }
    if let Some(avoid) = guidance
        .avoid
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        lines.push("## Failure modes to avoid".to_string());
        lines.push(String::new());
        lines.push(avoid.to_string());
        lines.push(String::new());
    }

    format!("{}\n", lines.join("\n"))
}

/// Render the opening `## Situation` block — loop semantics, the mutation
/// mandate, the context-budget signal, and the paired-process-mem line.
/// Byte-for-byte the plugin's `situationBlock`.
pub fn render_situation(resolved: &ResolvedIngest, process_mem: &ProcessMemInfo) -> String {
    let mode = mode_label(resolved.mode);
    let name = &resolved.name;
    let mut lines: Vec<String> = Vec::new();
    lines.push("## Situation".to_string());
    lines.push(String::new());
    lines.push(format!(
        "You are running one iteration of `{name}` ({mode} mode) inside a loop. \
         Each iteration is a fresh agent with no memory of prior runs; the destination \
         graph persists between runs and is your continuity. Backoff is mechanical — \
         when nothing has changed since the last run, the loop skips this ingest silently. \
         Reporting \"no changes\" is therefore a valid outcome."
    ));
    lines.push(String::new());
    lines.push(
        "Mutating the destination is this run's mandate: within the destination mem(s) and \
         paired process mem named under Operative data, create, update, relate, and delete \
         entities without asking. Project-level instructions that make entity creation/deletion \
         ask-first govern interactive dev sessions, not ingest iterations — parking creatable \
         work as a coverage_gap because of that rule defeats the loop. Mems outside the declared \
         destinations remain off-limits."
            .to_string(),
    );
    lines.push(String::new());
    lines.push(
        "Context budget is finite. The `PreCompact` hook fires near the limit and asks you to \
         stop and report. Multiple cycles inside one run are fine when context allows; depth on \
         a coherent area beats breadth across unrelated ones."
            .to_string(),
    );
    lines.push(String::new());
    if process_mem.present {
        lines.push(format!(
            "A paired process mem `{}` (schema `{PROCESS_MEM_SCHEMA}`) carries destination-quality \
             debt prior runs could not address. Its entries are objective claims about destination \
             state — read them on orientation, write to it when this run also cannot fix some debt, \
             delete entries the destination has since resolved. Call \
             `memstead_schema(name={PROCESS_MEM_SCHEMA})` once for the type vocabulary and write rules.",
            process_mem.mem_label
        ));
    } else if let Some(notice) = &process_mem.notice {
        lines.push(format!(
            "Note: paired process mem `{}` could not be auto-created — {notice}. The run continues \
             without it; the operator can retry with `memstead mem init {name} --org-path ingest \
             --schema {PROCESS_MEM_SCHEMA}`.",
            process_mem.mem_label
        ));
    } else if process_mem.skipped {
        lines.push(format!(
            "No process mem is paired with this ingest (mode={mode}; one-shot ingests are \
             by-design ephemeral)."
        ));
    }
    lines.push(String::new());
    format!("{}\n", lines.join("\n"))
}

/// Render the `## About the source` block from the projection's intent, or
/// the empty string when there is no intent. Byte-for-byte the plugin's
/// `intentBlock`.
pub fn render_intent(resolved: &ResolvedIngest) -> String {
    match resolved
        .intent
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(intent) => format!("## About the source\n\n{intent}\n\n"),
        None => String::new(),
    }
}

/// Render the `## Operative data` block — the sources (with their scope), the
/// destination (with its schema), and the paired process mem. Byte-for-byte
/// the plugin's `operativeDataBlock`. `destination_schema` is the schema ref
/// the destination mem pins (from `memMeta`), rendered when present.
///
/// A source facet's `domains` (web mediums) is not rendered — the engine's
/// facet scope models allow/deny paths only; the domains slot lands with web
/// medium support.
pub fn render_operative_data(
    resolved: &ResolvedIngest,
    process_mem: &ProcessMemInfo,
    destination_schema: Option<&str>,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("## Operative data".to_string());
    lines.push(String::new());

    // Sources
    if !resolved.sources.is_empty() {
        lines.push("### Sources".to_string());
        lines.push(String::new());
        let mut reference_mems: Vec<String> = Vec::new();
        for source in &resolved.sources {
            match source {
                ResolvedSource::Primary(p) => {
                    lines.push(format!(
                        "- **{}** (primary)",
                        medium_type_label(p.medium_type)
                    ));
                    let allows: Vec<&str> = p
                        .scope
                        .iter()
                        .filter(|r| r.mode == PatternMode::Allow)
                        .map(|r| r.path.as_str())
                        .collect();
                    let denies: Vec<&str> = p
                        .scope
                        .iter()
                        .filter(|r| r.mode == PatternMode::Deny)
                        .map(|r| r.path.as_str())
                        .collect();
                    if !allows.is_empty() {
                        lines.push(format!("  - Paths: {}", allows.join(", ")));
                    }
                    if !denies.is_empty() {
                        lines.push(format!("  - Ignore: {}", denies.join(", ")));
                    }
                }
                ResolvedSource::Reference { mem } => {
                    lines.push(format!("- **graph** (reference) — mem: {mem}"));
                    reference_mems.push(mem.clone());
                }
            }
        }
        lines.push(String::new());
        if !reference_mems.is_empty() {
            lines.push(
                "Sources tagged `(reference)` are read-only context for cross-mem edges — search \
                 them, never write into them. Only `(primary)` sources are ingested into the \
                 destination."
                    .to_string(),
            );
            lines.push(String::new());
            let mem_list = reference_mems
                .iter()
                .map(|v| format!("`memstead_search mem={v}`"))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!(
                "**Cross-mem references:** consult {mem_list} before authoring cross-mem edges. \
                 The target entity must exist — a wiki-link or relationship to a missing target \
                 either auto-stubs (silent) or fails authorization (`CROSS_MEM_RELATION`)."
            ));
            lines.push(String::new());
        }
    }

    // Destination — four-primitive projections carry exactly one, no role.
    lines.push("### Destination".to_string());
    lines.push(String::new());
    let schema_bit = destination_schema
        .map(|s| format!(" — schema: `{s}`"))
        .unwrap_or_default();
    lines.push(format!("- **{}**{schema_bit}", resolved.destination_mem));
    lines.push(String::new());

    // Paired process mem
    if process_mem.present {
        lines.push("### Paired process mem".to_string());
        lines.push(String::new());
        lines.push(format!(
            "- **{}** — schema: `{PROCESS_MEM_SCHEMA}`. Inspect via `memstead_overview` / \
             `memstead_search mem={}`.",
            process_mem.mem_label, process_mem.leaf_name
        ));
        lines.push(String::new());
    }

    format!("{}\n", lines.join("\n"))
}

/// One `memstead mem set-sync-state` argument pair — the durable baseline a
/// facet's cursor advances to after a full pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCommand {
    /// The sync-state key, conventionally `"<ingest>/<facet>"`.
    pub key: String,
    /// The opaque new-baseline token.
    pub token: String,
}

/// The combined source-cursor across a projection's source facets — the
/// engine-side of the plugin's `cursor` object that `changedSliceBlock`
/// consumes. Assembled by [`super::cursor::compute_source_cursor`] from the
/// per-facet [`super::slice::SliceOutcome`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCursor {
    /// The combined changed slice across all source facets.
    pub union: Slice,
    /// New-baseline commands for facets that changed.
    pub write_commands: Vec<SyncCommand>,
    /// New-baseline commands for facets seen for the first time (reseed).
    pub reseed: Vec<SyncCommand>,
    /// Whether any facet reported changes (drives the "source moved" copy).
    pub any_changes: bool,
    /// Whether any facet's slice was degraded (mtime memo miss → full scan).
    pub degraded: bool,
    /// The destination mem the `set-sync-state` commands target.
    pub dest_mem: String,
}

/// Single-quote a value for the emitted shell command, escaping embedded
/// single quotes. The digest token is JSON (contains `"` and `:`), so it
/// must be quoted to survive the shell. Mirrors the plugin's `shellQuote`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Render one changed-slice class (Deleted / Modified / Added), capped at
/// [`SLICE_CAP`] with a `…and N more` overflow line.
fn render_slice_class(lines: &mut Vec<String>, label: &str, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let shown = paths.len().min(SLICE_CAP);
    lines.push(format!("**{label}:**"));
    for path in &paths[..shown] {
        lines.push(format!("- `{path}`"));
    }
    if paths.len() > shown {
        lines.push(format!(
            "- …and {} more {}",
            paths.len() - shown,
            label.to_lowercase()
        ));
    }
    lines.push(String::new());
}

/// Render the `## Source changes since the last sync` preface — the changed
/// slice to steer at first, plus the `set-sync-state` "record the baseline
/// LAST" section. Byte-for-byte the plugin's `changedSliceBlock`. Returns the
/// empty string when nothing changed and nothing needs reseeding (making the
/// discovery brief byte-identical to a plain roam).
pub fn render_changed_slice(cursor: &SourceCursor) -> String {
    if !cursor.any_changes && cursor.reseed.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    lines.push("## Source changes since the last sync\n".to_string());

    if cursor.any_changes {
        lines.push(
            "The source moved since this graph was last synced. Steer this pass at these changed \
             artifacts **first** — they are where the graph is most likely now wrong.\n"
                .to_string(),
        );
        // Deletions first — cheapest, highest-signal drift.
        render_slice_class(&mut lines, "Deleted", &cursor.union.deleted);
        render_slice_class(&mut lines, "Modified", &cursor.union.modified);
        render_slice_class(&mut lines, "Added", &cursor.union.added);
        if cursor.degraded {
            lines.push(
                "_(Precise change history for one or more facets was unavailable, so its full \
                 current file set is listed above. Detection still fired from the durable baseline; \
                 targeting is coarser this pass only.)_\n"
                    .to_string(),
            );
        }
    }

    if !cursor.reseed.is_empty() {
        let keys = cursor
            .reseed
            .iter()
            .map(|r| format!("`{}`", r.key))
            .collect::<Vec<_>>()
            .join(", ");
        let it = if cursor.reseed.len() == 1 {
            "it"
        } else {
            "them"
        };
        lines.push(format!(
            "No prior sync baseline exists for {keys} — treating the current source state as the \
             baseline (first sync). No priority slice from {it} this pass; proceed as usual.\n"
        ));
    }

    // Cursor-write instruction — recorded by the agent as the FINAL step, so
    // an interrupted pass leaves the baseline unchanged and re-presents the
    // same slice next run. Routed through the engine CLI — never a raw write.
    let all_commands: Vec<&SyncCommand> = cursor
        .write_commands
        .iter()
        .chain(cursor.reseed.iter())
        .collect();
    if !all_commands.is_empty() {
        lines.push("### Recording the new baseline (do this LAST)\n".to_string());
        lines.push(
            "Only after you have fully worked the changed artifacts above — and only if this pass \
             was not cut short — record the source state you synced against, so the next pass \
             targets just what changes next. Run, exactly once each:\n"
                .to_string(),
        );
        lines.push("```sh".to_string());
        for c in &all_commands {
            lines.push(format!(
                "memstead mem set-sync-state {} {} {}",
                cursor.dest_mem,
                shell_quote(&c.key),
                shell_quote(&c.token)
            ));
        }
        lines.push("```".to_string());
        lines.push(
            "If you were interrupted before finishing, skip this — leaving the baseline where it \
             is re-presents the same slice next run.\n"
                .to_string(),
        );
    }

    format!("{}\n", lines.join("\n"))
}

/// Assemble the discovery-mode brief — situation, about-the-source, goal/avoid,
/// operative-data, and the changed-slice preface — concatenating the truthy
/// blocks, matching the plugin's `parts.filter(Boolean).join('')`.
/// `changed_slice_preface` is the rendered changed-slice block (empty when
/// the source has not moved, making the brief byte-identical to a plain roam).
pub fn assemble_discovery_brief(
    resolved: &ResolvedIngest,
    guidance: &ResolvedGuidance,
    process_mem: &ProcessMemInfo,
    destination_schema: Option<&str>,
    changed_slice_preface: &str,
) -> String {
    let parts = [
        render_situation(resolved, process_mem),
        render_intent(resolved),
        render_goal_and_avoid(guidance),
        render_operative_data(resolved, process_mem, destination_schema),
        changed_slice_preface.to_string(),
    ];
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("")
}

/// Assemble the refinement brief — the discovery-style header (situation,
/// about-the-source, goal/avoid, operative-data, changed-slice) plus the
/// scout-or-writer `phase_block` appended. Mirrors the plugin's refinement
/// `parts`.
pub fn assemble_refinement_brief(
    resolved: &ResolvedIngest,
    guidance: &ResolvedGuidance,
    process_mem: &ProcessMemInfo,
    destination_schema: Option<&str>,
    changed_slice_preface: &str,
    phase_block: &str,
) -> String {
    let parts = [
        render_situation(resolved, process_mem),
        render_intent(resolved),
        render_goal_and_avoid(guidance),
        render_operative_data(resolved, process_mem, destination_schema),
        changed_slice_preface.to_string(),
        phase_block.to_string(),
    ];
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("")
}

/// Render the `## Mode: one-shot — lens routing` block — the destination-set
/// table, optional routing rule, idempotency contract, end-of-run report
/// template, and optional archive note. Byte-for-byte the plugin's
/// `oneShotLensBlock`. `destination_schema` / `destination_purpose` describe
/// the ingest's single destination (four-primitive projections have one).
pub fn render_one_shot_lens(
    resolved: &ResolvedIngest,
    destination_schema: Option<&str>,
    destination_purpose: Option<&str>,
) -> String {
    let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    let mut lines: Vec<String> = vec![
        "## Mode: one-shot — lens routing".to_string(),
        String::new(),
        "A lens iterates entities once and writes per-destination, then exits. The agent decides \
         per-entity which destinations to target (Routing rule). Re-runs use `memstead_update`; \
         never duplicate."
            .to_string(),
        String::new(),
    ];

    lines.push("### Destination set".to_string());
    lines.push(String::new());
    lines.push("| Mem | Schema | Purpose |".to_string());
    lines.push("|-------|--------|---------|".to_string());
    let schema = destination_schema.unwrap_or("(none)");
    let purpose = destination_purpose
        .filter(|s| !s.is_empty())
        .unwrap_or("(no purpose declared)");
    lines.push(format!(
        "| {} | {} | {} |",
        cell(&resolved.destination_mem),
        cell(schema),
        cell(purpose)
    ));
    lines.push(String::new());

    if let Some(routing) = resolved
        .rules
        .as_ref()
        .and_then(|r| r.get("routing"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        lines.push("### Routing rule".to_string());
        lines.push(String::new());
        lines.push("```".to_string());
        lines.push(routing.to_string());
        lines.push("```".to_string());
        lines.push(String::new());
    }

    lines.push("### Idempotency".to_string());
    lines.push(String::new());
    lines.push("- Search the destination before writing; route changes through `memstead_update` against the existing entity if present.".to_string());
    lines.push("- Skip writes when the lifted content matches what is already there (record as `skipped: already-up-to-date`).".to_string());
    lines.push(
        "- Use `memstead_create` only when no entity for that concept exists yet.".to_string(),
    );
    lines.push(String::new());

    lines.push("### End-of-run report".to_string());
    lines.push(String::new());
    lines.push("After every destination is processed, emit one block per destination on stdout, in Destination-set order:".to_string());
    lines.push(String::new());
    lines.push("```".to_string());
    lines.push(format!("### Report: {}", resolved.name));
    lines.push(String::new());
    lines.push("Destination: <mem>".to_string());
    lines.push("  created: <count>".to_string());
    lines.push("  updated: <count>".to_string());
    lines.push("  skipped: <count>".to_string());
    lines.push("  failed:  <count>".to_string());
    lines.push("  failures:".to_string());
    lines.push("    - <entity-key>: <error verbatim>".to_string());
    lines.push("  skipped-detail:".to_string());
    lines.push("    - <entity-key>: <one-line reason>".to_string());
    lines.push("```".to_string());
    lines.push(String::new());
    lines.push("Per-destination commits are independent — partial success is the accepted failure mode. No rollback.".to_string());
    lines.push(String::new());

    let archive = resolved
        .post_actions
        .as_ref()
        .and_then(|p| p.get("archive_source"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if archive {
        lines.push("### Archive after run".to_string());
        lines.push(String::new());
        lines.push("After the report has been emitted, archive the source planning mem — `post_actions.archive_source` is set on this ingest.".to_string());
        lines.push(String::new());
    }

    format!("{}\n", lines.join("\n"))
}

/// Assemble the one-shot brief — situation, about-the-source, goal/avoid,
/// operative-data, and the lens-routing block. Mirrors the plugin's one-shot
/// `parts`. A one-shot ingest has no paired process mem, so `process_mem`
/// should carry `skipped = true`.
pub fn assemble_one_shot_brief(
    resolved: &ResolvedIngest,
    guidance: &ResolvedGuidance,
    process_mem: &ProcessMemInfo,
    destination_schema: Option<&str>,
    destination_purpose: Option<&str>,
) -> String {
    let parts = [
        render_situation(resolved, process_mem),
        render_intent(resolved),
        render_goal_and_avoid(guidance),
        render_operative_data(resolved, process_mem, destination_schema),
        render_one_shot_lens(resolved, destination_schema, destination_purpose),
    ];
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::resolve::ResolvedPrimarySource;
    use crate::pipeline::{IngestTrigger, PatternEntry};

    fn guidance(goal: Option<&str>, avoid: Option<&str>) -> ResolvedGuidance {
        ResolvedGuidance {
            goal: goal.map(str::to_string),
            avoid: avoid.map(str::to_string),
        }
    }

    /// Goal and avoid both present: two headers, trimmed prose, block ends in
    /// a blank line — byte-for-byte the plugin's `goalAndAvoidBlock`.
    #[test]
    fn renders_goal_and_avoid_blocks() {
        let out = render_goal_and_avoid(&guidance(Some("  build coverage  "), Some("no stubs")));
        assert_eq!(
            out,
            "## Goal\n\nbuild coverage\n\n## Failure modes to avoid\n\nno stubs\n\n"
        );
    }

    /// Goal only: a single header block ending in a blank line.
    #[test]
    fn renders_goal_only() {
        assert_eq!(
            render_goal_and_avoid(&guidance(Some("build coverage"), None)),
            "## Goal\n\nbuild coverage\n\n"
        );
    }

    /// Avoid only: a single header block ending in a blank line.
    #[test]
    fn renders_avoid_only() {
        assert_eq!(
            render_goal_and_avoid(&guidance(None, Some("no stubs"))),
            "## Failure modes to avoid\n\nno stubs\n\n"
        );
    }

    /// Neither present (and no pass-through): a lone newline, matching the
    /// plugin's `lines.join('\n') + '\n'` on an empty block.
    #[test]
    fn empty_guidance_yields_a_newline() {
        assert_eq!(render_goal_and_avoid(&guidance(None, None)), "\n");
        // An all-whitespace field is treated as absent.
        assert_eq!(render_goal_and_avoid(&guidance(Some("   "), None)), "\n");
    }

    fn primary(medium_type: MediumType, scope: Vec<PatternEntry>) -> ResolvedSource {
        ResolvedSource::Primary(ResolvedPrimarySource {
            facet_ref: "f".to_string(),
            medium: "m".to_string(),
            medium_type,
            medium_pointer: "../src".to_string(),
            declared_change_detection: None,
            scope,
            preparation: None,
        })
    }

    fn resolved(name: &str, intent: Option<&str>, sources: Vec<ResolvedSource>) -> ResolvedIngest {
        ResolvedIngest {
            name: name.to_string(),
            mode: IngestMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 20,
            deny_paths: vec![],
            projection_ref: format!("{name}/p"),
            projection_mem: name.to_string(),
            projection_name: "p".to_string(),
            intent: intent.map(str::to_string),
            sources,
            destination_mem: name.to_string(),
            rules: None,
            post_actions: None,
        }
    }

    fn process_present(name: &str) -> ProcessMemInfo {
        ProcessMemInfo {
            present: true,
            skipped: false,
            notice: None,
            leaf_name: name.to_string(),
            mem_label: format!("ingest/{name}"),
        }
    }

    fn allow(path: &str) -> PatternEntry {
        PatternEntry {
            path: path.to_string(),
            mode: PatternMode::Allow,
        }
    }

    fn deny(path: &str) -> PatternEntry {
        PatternEntry {
            path: path.to_string(),
            mode: PatternMode::Deny,
        }
    }

    /// The about-the-source block trims the intent; no intent → empty string.
    #[test]
    fn renders_intent() {
        let r = resolved("macos", Some("  Swift app source.  "), vec![]);
        assert_eq!(
            render_intent(&r),
            "## About the source\n\nSwift app source.\n\n"
        );
        let none = resolved("macos", None, vec![]);
        assert_eq!(render_intent(&none), "");
    }

    /// The situation block prints the name/mode, the three fixed paragraphs,
    /// and the present-process-mem line, ending in a blank line.
    #[test]
    fn renders_situation_with_present_process_mem() {
        let r = resolved("macos", None, vec![]);
        let out = render_situation(&r, &process_present("macos"));
        assert!(out.starts_with("## Situation\n\nYou are running one iteration of `macos` (discovery mode) inside a loop."));
        assert!(out.contains("Mutating the destination is this run's mandate:"));
        assert!(out.contains("The `PreCompact` hook fires near the limit"));
        assert!(out.contains("A paired process mem `ingest/macos` (schema `ingest@0.1.0`) carries destination-quality debt"));
        assert!(
            out.ends_with("write rules.\n\n"),
            "block ends in a blank line"
        );
    }

    /// The skipped (one-shot) and failed-to-create process-mem branches each
    /// render their own note.
    #[test]
    fn situation_process_mem_branches() {
        let mut r = resolved("os", None, vec![]);
        r.mode = IngestMode::OneShot;
        let skipped = ProcessMemInfo {
            present: false,
            skipped: true,
            notice: None,
            leaf_name: "os".to_string(),
            mem_label: "ingest/os".to_string(),
        };
        assert!(
            render_situation(&r, &skipped)
                .contains("No process mem is paired with this ingest (mode=one-shot;")
        );

        let failed = ProcessMemInfo {
            present: false,
            skipped: false,
            notice: Some("engine offline".to_string()),
            leaf_name: "os".to_string(),
            mem_label: "ingest/os".to_string(),
        };
        let out = render_situation(&resolved("os", None, vec![]), &failed);
        assert!(out.contains("could not be auto-created — engine offline."));
        assert!(out.contains("memstead mem init os --org-path ingest --schema ingest@0.1.0"));
    }

    /// Operative data: a primary source with paths/ignore, a reference mem
    /// with its cross-mem note, the destination with its schema, and the
    /// paired process mem — byte-for-byte the plugin's block.
    #[test]
    fn renders_operative_data_full() {
        let r = resolved(
            "macos",
            None,
            vec![
                primary(
                    MediumType::Codebase,
                    vec![allow("src/**/*.swift"), deny("src/gen/**")],
                ),
                ResolvedSource::Reference {
                    mem: "engine".to_string(),
                },
            ],
        );
        let out = render_operative_data(&r, &process_present("macos"), Some("macos-code@0.1.0"));
        let expected = "\
## Operative data

### Sources

- **codebase** (primary)
  - Paths: src/**/*.swift
  - Ignore: src/gen/**
- **graph** (reference) — mem: engine

Sources tagged `(reference)` are read-only context for cross-mem edges — search them, never write into them. Only `(primary)` sources are ingested into the destination.

**Cross-mem references:** consult `memstead_search mem=engine` before authoring cross-mem edges. The target entity must exist — a wiki-link or relationship to a missing target either auto-stubs (silent) or fails authorization (`CROSS_MEM_RELATION`).

### Destination

- **macos** — schema: `macos-code@0.1.0`

### Paired process mem

- **ingest/macos** — schema: `ingest@0.1.0`. Inspect via `memstead_overview` / `memstead_search mem=macos`.
\n";
        assert_eq!(out, expected);
    }

    /// Operative data without references or a destination schema: no cross-mem
    /// note, a bare destination line.
    #[test]
    fn renders_operative_data_minimal() {
        let r = resolved("g", None, vec![primary(MediumType::Filesystem, vec![])]);
        let skipped = ProcessMemInfo {
            present: false,
            skipped: true,
            notice: None,
            leaf_name: "g".to_string(),
            mem_label: "ingest/g".to_string(),
        };
        let out = render_operative_data(&r, &skipped, None);
        assert!(out.contains("- **filesystem** (primary)\n"));
        assert!(!out.contains("Cross-mem references"), "no reference note");
        assert!(out.contains("### Destination\n\n- **g**\n"));
        assert!(
            !out.contains("Paired process mem"),
            "skipped process mem omitted"
        );
    }

    /// The discovery assembly concatenates the truthy blocks in order; an
    /// empty changed-slice preface (source unmoved) drops out.
    #[test]
    fn assembles_discovery_brief() {
        let r = resolved(
            "macos",
            Some("Swift source."),
            vec![primary(MediumType::Codebase, vec![allow("src/**")])],
        );
        let g = guidance(Some("build coverage"), None);
        let pm = process_present("macos");
        let brief = assemble_discovery_brief(&r, &g, &pm, Some("s@1"), "");

        // Blocks appear in order and the empty preface is dropped.
        let sit = brief.find("## Situation").unwrap();
        let src = brief.find("## About the source").unwrap();
        let goal = brief.find("## Goal").unwrap();
        let op = brief.find("## Operative data").unwrap();
        assert!(
            sit < src && src < goal && goal < op,
            "blocks in brief order"
        );
        assert!(
            !brief.contains("## Source changes"),
            "no changed-slice block when preface empty"
        );

        // A non-empty preface is appended verbatim at the end.
        let with_slice =
            assemble_discovery_brief(&r, &g, &pm, Some("s@1"), "## Source changes\n\n…\n\n");
        assert!(with_slice.ends_with("## Source changes\n\n…\n\n"));
    }

    fn slice(deleted: &[&str], modified: &[&str], added: &[&str]) -> Slice {
        Slice {
            deleted: deleted.iter().map(|s| s.to_string()).collect(),
            modified: modified.iter().map(|s| s.to_string()).collect(),
            added: added.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn cmd(key: &str, token: &str) -> SyncCommand {
        SyncCommand {
            key: key.to_string(),
            token: token.to_string(),
        }
    }

    /// No changes and no reseed → the block is empty (brief stays a plain roam).
    #[test]
    fn changed_slice_empty_when_nothing_moved() {
        let cursor = SourceCursor {
            union: slice(&[], &[], &[]),
            write_commands: vec![],
            reseed: vec![],
            any_changes: false,
            degraded: false,
            dest_mem: "engine".to_string(),
        };
        assert_eq!(render_changed_slice(&cursor), "");
    }

    /// A changed pass renders deleted-first, then the recording block — built
    /// here from single-line literals transcribed from the plugin so any
    /// line-continuation drift in the impl is caught.
    #[test]
    fn changed_slice_renders_slice_and_recording() {
        let cursor = SourceCursor {
            union: slice(&["a.rs"], &["b.rs"], &[]),
            write_commands: vec![cmd("engine-graph/source", "HEADSHA")],
            reseed: vec![],
            any_changes: true,
            degraded: false,
            dest_mem: "engine".to_string(),
        };
        let expected_lines = [
            "## Source changes since the last sync\n",
            "The source moved since this graph was last synced. Steer this pass at these changed artifacts **first** — they are where the graph is most likely now wrong.\n",
            "**Deleted:**",
            "- `a.rs`",
            "",
            "**Modified:**",
            "- `b.rs`",
            "",
            "### Recording the new baseline (do this LAST)\n",
            "Only after you have fully worked the changed artifacts above — and only if this pass was not cut short — record the source state you synced against, so the next pass targets just what changes next. Run, exactly once each:\n",
            "```sh",
            "memstead mem set-sync-state engine 'engine-graph/source' 'HEADSHA'",
            "```",
            "If you were interrupted before finishing, skip this — leaving the baseline where it is re-presents the same slice next run.\n",
        ];
        assert_eq!(
            render_changed_slice(&cursor),
            format!("{}\n", expected_lines.join("\n"))
        );
    }

    /// The reseed-only path names the first-sync keys and still emits the
    /// recording block (the reseed baselines).
    #[test]
    fn changed_slice_reseed_only() {
        let cursor = SourceCursor {
            union: slice(&[], &[], &[]),
            write_commands: vec![],
            reseed: vec![cmd("ing/f", "TOK")],
            any_changes: false,
            degraded: false,
            dest_mem: "d".to_string(),
        };
        let out = render_changed_slice(&cursor);
        assert!(out.starts_with("## Source changes since the last sync\n\n"));
        assert!(out.contains(
            "No prior sync baseline exists for `ing/f` — treating the current source state as the baseline (first sync). No priority slice from it this pass; proceed as usual."
        ));
        assert!(out.contains("memstead mem set-sync-state d 'ing/f' 'TOK'"));
        assert!(
            !out.contains("The source moved"),
            "no 'moved' copy when only reseeding"
        );
    }

    /// The one-shot lens block: destination-set table, routing rule (when set),
    /// idempotency, report template, and archive note (when set).
    #[test]
    fn renders_one_shot_lens_block() {
        let mut r = resolved("os", Some("plan source"), vec![]);
        r.rules = Some(serde_json::json!({ "routing": "route each entity to its spec" }));
        r.post_actions = Some(serde_json::json!({ "archive_source": true }));

        let out = render_one_shot_lens(&r, Some("planning@0.1.0"), Some("the plan graph"));
        assert!(out.starts_with("## Mode: one-shot — lens routing\n\n"));
        assert!(out.contains(
            "### Destination set\n\n| Mem | Schema | Purpose |\n|-------|--------|---------|\n| os | planning@0.1.0 | the plan graph |\n"
        ));
        assert!(out.contains("### Routing rule\n\n```\nroute each entity to its spec\n```\n"));
        assert!(out.contains("### Idempotency"));
        assert!(out.contains("### Report: os"));
        assert!(out.contains("### Archive after run"));
        assert!(out.ends_with("is set on this ingest.\n\n"));

        // No routing / no archive → those sections are omitted; a bare schema
        // and default purpose fall back.
        let bare = resolved("os", None, vec![]);
        let out2 = render_one_shot_lens(&bare, None, None);
        assert!(out2.contains("| os | (none) | (no purpose declared) |"));
        assert!(!out2.contains("### Routing rule"));
        assert!(!out2.contains("### Archive after run"));
        assert!(out2.contains("### End-of-run report"));
    }

    /// The one-shot brief assembles situation (one-shot mode) + intent +
    /// goal/avoid + operative-data + the lens block; no process mem, no slice.
    #[test]
    fn assembles_one_shot_brief() {
        let mut r = resolved(
            "os",
            Some("src"),
            vec![primary(MediumType::Filesystem, vec![])],
        );
        r.mode = IngestMode::OneShot;
        let g = guidance(Some("goal"), None);
        let skipped = ProcessMemInfo {
            present: false,
            skipped: true,
            notice: None,
            leaf_name: "os".to_string(),
            mem_label: "ingest/os".to_string(),
        };
        let brief = assemble_one_shot_brief(&r, &g, &skipped, Some("s@1"), Some("purpose"));
        assert!(brief.contains("(one-shot mode)"));
        assert!(brief.contains("No process mem is paired with this ingest (mode=one-shot;"));
        assert!(brief.contains("## Mode: one-shot — lens routing"));
        assert!(
            !brief.contains("## Source changes"),
            "one-shot has no changed-slice"
        );
    }

    /// Beyond SLICE_CAP entries an overflow line stands in; the degraded flag
    /// adds the coarse-targeting note. Also exercises shell-quoting a JSON
    /// digest token (embedded quotes).
    #[test]
    fn changed_slice_caps_and_degrades_and_quotes() {
        let many: Vec<String> = (0..SLICE_CAP + 3).map(|i| format!("f{i}.rs")).collect();
        let cursor = SourceCursor {
            union: Slice {
                deleted: vec![],
                modified: vec![],
                added: many,
            },
            write_commands: vec![cmd("ing/f", r#"{"v":1,"aggregate":"x"}"#)],
            reseed: vec![],
            any_changes: true,
            degraded: true,
            dest_mem: "d".to_string(),
        };
        let out = render_changed_slice(&cursor);
        assert!(out.contains(&format!("- …and {} more added", 3)));
        assert!(out.contains("Precise change history for one or more facets was unavailable"));
        // The JSON token is single-quoted; its embedded `"` survive unescaped.
        assert!(out.contains(r#"'{"v":1,"aggregate":"x"}'"#));
    }
}
