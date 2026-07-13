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
//! and [`assemble_one_shot_brief`] — plus the
//! changed-slice preface ([`render_changed_slice`], rendered from a
//! [`SourceCursor`]).

use super::guidance::ResolvedGuidance;
use super::resolve::{ResolvedIngest, ResolvedSource};
use super::slice::{NoSignalReason, Slice};
use crate::binding::BuildMode;
use crate::pipeline::{MediumType, PatternMode};

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

/// The mode string the situation block prints (`discovery` / `one-shot`) —
/// the same tokens the plugin uses.
fn mode_label(mode: BuildMode) -> &'static str {
    match mode {
        BuildMode::Discovery => "discovery",
        BuildMode::OneShot => "one-shot",
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

/// One baseline token a facet's cursor advances to after a full pass — the
/// `(sync_state key, medium-typed token)` pair the engine records via the
/// `set_mem_sync_state` writer. Produced by the cursor; the brief no longer
/// renders it as an operator command (the agent runs `projection advance`,
/// which computes and records the token engine-side — D4/D7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCommand {
    /// The sync-state key, `"<binding-id>/<facet>#synced"` (D4).
    pub key: String,
    /// The opaque new-baseline token.
    pub token: String,
}

/// A source whose change detection produced **no usable signal** this pass,
/// with the classified [`NoSignalReason`]. Rendered as a distinct per-source
/// note in the changed-slice preface, so the agent can tell a *blind* source
/// (no baseline comparison happened) from a *genuinely-unchanged* one (checked,
/// did not move — which stays silent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoSignalNote {
    /// The source's label — the facet ref (primary) or mem id (reference), the
    /// same token the `<ingest>/<label>` sync-state key is built from.
    pub source: String,
    /// Why detection produced no signal.
    pub reason: NoSignalReason,
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
    /// Per-source no-signal notes — sources whose detection could not produce a
    /// slice this pass (unscoped facet, `signal:none`, git failure, missing
    /// graph snapshot). Rendered distinctly from changed/reseed; a
    /// genuinely-unchanged source contributes nothing here, so an all-unchanged
    /// brief still renders no preface (byte-identical to a plain roam).
    pub no_signal: Vec<NoSignalNote>,
    /// Whether any facet reported changes (drives the "source moved" copy).
    pub any_changes: bool,
    /// Whether any facet's slice was degraded (mtime memo miss → full scan).
    pub degraded: bool,
    /// Ingest `deny_paths` entries that matched **no file** anywhere the agent
    /// can reach (the project tree). A zero-selecting deny is surfaced as a
    /// rendered warning rather than silently no-op'ing — it catches typos and
    /// un-migrated legacy bare names (which, as globs, match nothing). Never a
    /// hard error: the ingest still runs, the entry just does nothing.
    pub dead_denies: Vec<String>,
    /// The destination mem whose `sync_state` the baseline tokens live on.
    pub dest_mem: String,
    /// The canonical binding id `<mem>/<stem>` (D3) — rendered into the
    /// `memstead projection advance <binding-id> …` line the changed-slice
    /// preface now emits instead of a raw `mem set-sync-state` command (D4/D7).
    pub binding_id: String,
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

/// The one-line explanation the brief prints for a [`NoSignalReason`] — each
/// reason renders as distinct text, so the agent can tell the no-signal
/// conditions apart (and all apart from a genuinely-unchanged source, which
/// renders nothing at all).
fn no_signal_reason_text(reason: NoSignalReason) -> &'static str {
    match reason {
        NoSignalReason::Unscoped => {
            "unscoped facet (no allow patterns) — nothing is monitored; write `**/*` in the \
             facet scope to watch the whole medium"
        }
        NoSignalReason::DetectionNone => {
            "`signal:none` — change detection is disabled for this source (declared `none`)"
        }
        NoSignalReason::GitUnavailable => {
            "git signal unavailable — no work tree, an unreadable `HEAD`, or an unknown baseline; \
             a full re-roam is warranted this pass"
        }
        NoSignalReason::GraphSnapshotMissing => {
            "graph snapshot missing — the source mem has no comparable baseline this pass"
        }
    }
}

/// Render the `## Source changes since the last sync` preface — the changed
/// slice to steer at first, any no-signal sources, plus the `projection advance`
/// "record your dispositions LAST" section. Extends the plugin's `changedSliceBlock`
/// with the no-signal notes. Returns the empty string when nothing changed,
/// nothing needs reseeding, and every source is genuinely unchanged (no
/// no-signal notes) — making the brief byte-identical to a plain roam.
pub fn render_changed_slice(cursor: &SourceCursor) -> String {
    if !cursor.any_changes
        && cursor.reseed.is_empty()
        && cursor.no_signal.is_empty()
        && cursor.dead_denies.is_empty()
    {
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

    if !cursor.no_signal.is_empty() {
        lines.push(
            "Some sources produced **no change signal** this pass — detection could not compare \
             them against a baseline, so they were not steered (roam them as usual). This is \
             distinct from a source that was checked and had not moved:\n"
                .to_string(),
        );
        for note in &cursor.no_signal {
            lines.push(format!(
                "- `{}`: {}",
                note.source,
                no_signal_reason_text(note.reason)
            ));
        }
        lines.push(String::new());
    }

    if !cursor.dead_denies.is_empty() {
        lines.push(
            "**Warning — some `deny_paths` entries match nothing.** The following ingest \
             `deny_paths` selected **no file** in the project tree, so they exclude nothing from \
             the slice and hide nothing from the ingest agent. This is usually a typo or a legacy \
             bare name that never migrated to the workspace-relative glob dialect (e.g. `dev` → \
             `dev/**`, `VISION.md` → `**/VISION.md`). Fix or remove them:\n"
                .to_string(),
        );
        for entry in &cursor.dead_denies {
            lines.push(format!("- `{entry}`"));
        }
        lines.push(String::new());
    }

    // Disposition-record instruction — the agent's FINAL step. The advance is
    // resumable and non-stalling (D7): a partial pass is honored on disk, and a
    // source that moves mid-pass re-presents (remaining + new) without losing
    // recorded work. The agent runs `projection advance`, which computes and
    // records the new baseline token engine-side — the brief no longer renders a
    // raw `mem set-sync-state` command (D4). The block appears whenever there is
    // a baseline to advance (a changed facet or a first-sync reseed).
    let has_baseline_to_advance = !cursor.write_commands.is_empty() || !cursor.reseed.is_empty();
    if has_baseline_to_advance {
        lines.push("### Recording your dispositions (do this LAST)\n".to_string());
        lines.push(
            "Only after you have worked the changed artifacts above — and only for the artifacts \
             you actually judged — record a disposition for each, so the next pass targets just \
             what changes next. This advance is resumable and non-stalling: a partial pass is \
             honored, and if the source moves mid-pass the remaining slice re-presents \
             (remaining + new) without losing your recorded work.\n"
                .to_string(),
        );
        lines.push(
            "Anchored work disposes itself: at advance time, every listed artifact that an \
             anchor in the destination mem references is marked `worked` automatically (an \
             explicit disposition you pass wins over the auto-mark). Supply dispositions only \
             for the residue — artifacts you skipped, judged out of intent, or worked without \
             anchors. The gate accepts only artifact ids listed above — an unknown id refuses \
             the whole call. When every artifact is disposed, the sync baseline advances \
             automatically. Run:\n"
                .to_string(),
        );
        lines.push("```sh".to_string());
        lines.push(format!(
            "memstead projection advance {} --dispositions {}",
            cursor.binding_id,
            shell_quote(r#"{"<artifact>": "<disposition>", ...}"#)
        ));
        lines.push("```".to_string());
        lines.push(
            "If you were interrupted before finishing, that is fine — your recorded dispositions \
             persist, and the next run re-presents only what is left.\n"
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
/// Render the `## Provenance — anchor your writes` block — the build-brief
/// instruction to attach `anchors[]` to every entity mutation. Rendered by the
/// engine, never by skill prose: a binary old enough to reject the parameter
/// never renders the instruction, so the brief cannot version-skew against its
/// own mutation surface (the reason the plugin-side capability gate exists for
/// skill-carried prose). The element shape is taught by the mutation tools'
/// own descriptions; the brief carries only the job.
pub fn render_anchor_instruction() -> String {
    "## Provenance — anchor your writes\n\n\
     Attach an `anchors` list to every `memstead_create` / `memstead_update`, naming the \
     source artifact(s) the entity is drawn from (the mutation tools document the element \
     shape). Anchored writes are what verify measures coverage and drift against, and — on \
     cursor-driven passes — what the advance gate auto-marks `worked`; an unanchored write \
     leaves the fidelity report and the disposition window blind to your work.\n\n"
        .to_string()
}

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
        render_anchor_instruction(),
        changed_slice_preface.to_string(),
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
        render_anchor_instruction(),
        render_one_shot_lens(resolved, destination_schema, destination_purpose),
    ];
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("")
}

// ---------------------------------------------------------------------------
// Verify + sync briefs (group C) — the measure/repair surface beside the build
// briefs. Verify MEASURES (no destination mutation of any kind, C1); sync is the
// SOLE maintenance writer, carrying BOTH the cursor slice and the open findings
// in one brief (C2) with the whole of `/reconcile`'s absorbed judgment (C3). A
// rule-by-rule absorption map records where each retired reconcile rule now
// lives (bundle plan `05-verify-sync-engine`, C4).
// ---------------------------------------------------------------------------

use super::findings::{Finding, FindingClass, FindingTarget};
use super::prune::{PruneDisposition, PruneProposal};

/// Per-class cap on the rendered open-findings list — mirrors [`SLICE_CAP`].
const FINDINGS_CAP: usize = SLICE_CAP;

/// Render the **verify brief** (C1) — the measurement + capped-adjudication
/// prompt an agent consumes to *measure* a binding's fidelity.
///
/// **Refusal (C1), structural:** this function emits **no destination-mutation
/// instruction of any kind**. It tells the agent what to measure and adjudicate,
/// never what to write into the destination mem — every repair is recorded as a
/// finding for the sync brief ([`render_sync_brief`]) to act on. There is no
/// create / update / relate / delete instruction anywhere in the rendered text.
pub fn render_verify_brief(resolved: &ResolvedIngest, backlog: usize) -> String {
    let mut lines: Vec<String> = vec![
        "## Verify — measure fidelity, do not mutate".to_string(),
        String::new(),
    ];
    lines.push(format!(
        "You are measuring the fidelity of `{}` — how faithfully the destination mem \
         `{}` still matches its source. This pass **only measures**: read the source \
         and the mem's anchors, judge whether the graph still holds, and record what \
         you find. Nothing here writes into the destination mem.",
        resolved.name, resolved.destination_mem
    ));
    lines.push(String::new());

    lines.push("### Adjudicate the queued findings (capped)".to_string());
    lines.push(String::new());
    if backlog == 0 {
        lines.push(
            "No findings are queued for adjudication this pass. Spot-check the resolving \
             anchors and the uncovered-artifact sample the fidelity report lists, and \
             record any drift you observe as a finding."
                .to_string(),
        );
    } else {
        lines.push(format!(
            "{backlog} finding(s) are queued for adjudication. Working up to the per-run \
             adjudication cap (an operations knob — the remainder stays queued and \
             re-presents on a later pass), take each queued finding and compare the \
             anchored source content against what the entity records. Classify it: still \
             accurate, or drifted. **Record the verdict — this is a measurement, not a \
             repair.** A drift you record becomes a finding the sync pass repairs; you do \
             not fix it here."
        ));
    }
    lines.push(String::new());

    lines.push("### Out of scope for verify — no mutation".to_string());
    lines.push(String::new());
    lines.push(
        "Verify writes **nothing** into the destination mem. Do not update a \
         `specifies` / `constraints` section, do not create or delete an entity, do not \
         add or remove a relationship. When measurement shows the graph is wrong, that \
         is a **finding** — the sync brief (`memstead projection brief --sync`) is the \
         one place those repairs are made. Leave every fix to it."
            .to_string(),
    );
    lines.push(String::new());

    format!("{}\n", lines.join("\n"))
}

/// A compact `entity → artifact` (or bare artifact) label for a finding target.
fn finding_target_label(target: &FindingTarget) -> String {
    match target {
        FindingTarget::Anchor { entity, artifact } => format!("`{entity}` → `{artifact}`"),
        FindingTarget::Artifact { artifact } => format!("`{artifact}`"),
    }
}

/// Render one class-grouped findings section, capped at [`FINDINGS_CAP`] with a
/// `…and N more` overflow line. Skips an empty group entirely.
fn render_findings_group(
    lines: &mut Vec<String>,
    heading: &str,
    guidance: &str,
    items: &[&Finding],
) {
    if items.is_empty() {
        return;
    }
    lines.push(format!("### {heading}"));
    lines.push(String::new());
    lines.push(guidance.to_string());
    lines.push(String::new());
    let shown = items.len().min(FINDINGS_CAP);
    for f in &items[..shown] {
        lines.push(format!(
            "- {} — {}",
            finding_target_label(&f.target),
            f.detail
        ));
    }
    if items.len() > shown {
        lines.push(format!("- …and {} more", items.len() - shown));
    }
    lines.push(String::new());
}

/// Render the open-findings block for the sync brief (C2) — the findings
/// `findings_store.current(key)` returned, grouped by class, each carrying the
/// conservative repair guidance the reconcile rules (C3) mandate. Empty string
/// when there are no open findings.
fn render_open_findings(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = vec![
        "## Open findings to repair".to_string(),
        String::new(),
        "The verify pass recorded these against the current source state. Repair them \
         conservatively (see the rules below); a finding you judge already correct needs \
         no write."
            .to_string(),
        String::new(),
    ];

    let group = |class: FindingClass| -> Vec<&Finding> {
        findings.iter().filter(|f| f.class == class).collect()
    };

    // Drifted / wrong — the anchored content changed: update only what moved
    // (conservatism rule "never rewrite unchanged sections").
    render_findings_group(
        &mut lines,
        "Drifted — the anchored content changed",
        "The source the entity describes moved. Update the affected section to match — \
         only the part that changed. If the entity is still accurate, leave it.",
        &group(FindingClass::Drifted),
    );
    render_findings_group(
        &mut lines,
        "Wrong — an adjudicated content mismatch",
        "Adjudication found the entity no longer matches its source. Correct the \
         mismatched section; do not rewrite what still holds.",
        &group(FindingClass::Wrong),
    );
    // Unresolvable anchor — the artifact is gone: delete only if the concept is
    // removed entirely (conservatism rule "no deletion unless concept removed").
    render_findings_group(
        &mut lines,
        "Unresolvable anchor — the artifact is gone",
        "The source artifact an anchor references is no longer present. Delete the entity \
         **only** if the concept is removed entirely; otherwise leave it. Concept-level \
         removals are a prune concern with its own never-clobber / conflict-flag rules — \
         do not delete on a hunch here.",
        &group(FindingClass::UnresolvableAnchor),
    );
    // Uncovered — a source artifact with no entity: create only for a clearly-new
    // concept (conservatism rule "no new entities unless clearly-new concept").
    render_findings_group(
        &mut lines,
        "Uncovered — a source artifact with no entity",
        "An in-scope source artifact has no anchor in the mem. Create an entity for it \
         **only** if it is a clearly-new concept with no existing entity; otherwise \
         extend the entity that already owns the concept, or leave it for a discovery \
         build.",
        &group(FindingClass::Uncovered),
    );
    // Queued — not yet adjudicated: verify owns these, not sync.
    render_findings_group(
        &mut lines,
        "Queued for adjudication — not yet judged",
        "These are not adjudicated yet — that is the verify pass's job, not sync's. \
         **Skip them here**; they become repairable only after verify classifies them as \
         drifted.",
        &group(FindingClass::QueuedForAdjudication),
    );

    format!("{}\n", lines.join("\n"))
}

/// Render the prune-proposals block for the sync brief (group F) — the deletion
/// proposals prune surfaced, each with its guarantee-appropriate treatment.
/// Empty string when there are no proposals.
///
/// **F3 / A5, structural:** every proposal here is exactly that — a *proposal*.
/// Nothing in this text (nor anywhere in the engine) deletes an entity; the
/// removal reaches the mem **only** when the agent acts on this brief through the
/// MCP mutation surface. `authored` entities never reach this block (prune
/// excludes them upstream); `derived` entities are flagged with their inputs,
/// never proposed for deletion.
fn render_prune_proposals(proposals: &[PruneProposal]) -> String {
    if proposals.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = vec![
        "## Prune — proposed removals (you decide; nothing is auto-deleted)".to_string(),
        String::new(),
        "The source removed the artifacts these entities describe. Each item below is a \
         **proposal**: prune writes nothing — you enact (or reject) the removal through the \
         normal MCP mutation surface. An `authored` entity is never proposed here; a `derived` \
         entity is flagged, never proposed for deletion."
            .to_string(),
        String::new(),
    ];

    let group = |d: PruneDisposition| -> Vec<&PruneProposal> {
        proposals.iter().filter(|p| p.disposition == d).collect()
    };

    // Clean-delete — never-clobber, base retrieved, merge clean: a confident
    // (still agent-enacted) delete proposal.
    let clean = group(PruneDisposition::CleanDelete);
    if !clean.is_empty() {
        lines.push("### Clean delete — never-clobber three-way merge is clean".to_string());
        lines.push(String::new());
        lines.push(
            "The source base leg was retrievable and the three-way merge found no model-side \
             divergence, so removal is safe. **Confirm, then delete via the mutation surface** — \
             this is still your call, not an auto-delete."
                .to_string(),
        );
        lines.push(String::new());
        let shown = clean.len().min(FINDINGS_CAP);
        for p in &clean[..shown] {
            lines.push(format!(
                "- `{}` — source artifact(s) gone: {}",
                p.entity,
                artifact_list(&p.artifacts)
            ));
        }
        if clean.len() > shown {
            lines.push(format!("- …and {} more", clean.len() - shown));
        }
        lines.push(String::new());
    }

    // Conflict-flag — both sides presented, never an auto-write over an edit.
    let conflict = group(PruneDisposition::ConflictFlag);
    if !conflict.is_empty() {
        lines.push("### Conflict-flag — decide, never overwrite a model-side edit".to_string());
        lines.push(String::new());
        lines.push(
            "No retrievable base leg to merge against (a non-git source, or an anchor with no \
             pinned version). **Both sides are shown — decide deliberately.** If the concept is \
             truly gone, delete via the mutation surface; if the model side was edited on \
             purpose, keep it. Prune never overwrites a model-side edit for you."
                .to_string(),
        );
        lines.push(String::new());
        let shown = conflict.len().min(FINDINGS_CAP);
        for p in &conflict[..shown] {
            lines.push(format!(
                "- `{}` — **source side:** artifact(s) gone: {}; **model side:** the entity is \
                 still present (may carry edits) — you decide.",
                p.entity,
                artifact_list(&p.artifacts)
            ));
        }
        if conflict.len() > shown {
            lines.push(format!("- …and {} more", conflict.len() - shown));
        }
        lines.push(String::new());
    }

    // Derived-flagged — flagged with inputs, never proposed for deletion (F3).
    let derived = group(PruneDisposition::DerivedFlagged);
    if !derived.is_empty() {
        lines.push("### Derived — flagged, NOT proposed for deletion".to_string());
        lines.push(String::new());
        lines.push(
            "These entities were **derived** from other inputs. A derived entity is flagged, \
             never auto-proposed for deletion — its inputs may still hold even though one source \
             artifact vanished. Re-examine the inputs before removing anything."
                .to_string(),
        );
        lines.push(String::new());
        let shown = derived.len().min(FINDINGS_CAP);
        for p in &derived[..shown] {
            let inputs = if p.derived_inputs.is_empty() {
                "(no recorded inputs)".to_string()
            } else {
                artifact_list(&p.derived_inputs)
            };
            lines.push(format!(
                "- `{}` — derived from: {}; source artifact(s) gone: {}.",
                p.entity,
                inputs,
                artifact_list(&p.artifacts)
            ));
        }
        if derived.len() > shown {
            lines.push(format!("- …and {} more", derived.len() - shown));
        }
        lines.push(String::new());
    }

    format!("{}\n", lines.join("\n"))
}

/// A compact backtick-joined artifact list.
fn artifact_list(artifacts: &[String]) -> String {
    if artifacts.is_empty() {
        return "(none)".to_string();
    }
    artifacts
        .iter()
        .map(|a| format!("`{a}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render the sync brief's `## Situation` block — the sole-maintenance-writer
/// mandate and the commits-nothing / engine-commits-per-mutation posture (C3).
fn render_sync_situation(resolved: &ResolvedIngest) -> String {
    format!(
        "## Sync — repair the graph to match the source\n\n\
         You are running the sync pass for `{}`. Sync is the graph's **sole maintenance \
         writer**: the only place the destination mem `{}` is repaired to match its \
         source. Two inputs steer this pass — the source changes since the last sync, and \
         the open verify findings — both below. Work them: update, create, relate, and \
         (rarely) delete entities so the graph again matches the source.\n\n\
         Every mutation routes through the normal MCP mutation surface, and the engine \
         commits each one **per-mutation** to the mem's own gitdir. You **stage nothing \
         and commit nothing yourself** — not the graph, not the code. Sync commits \
         nothing.\n\n",
        resolved.name, resolved.destination_mem
    )
}

/// Render the adopt / onboarding block (C3's first-sync/adopt framing; E1's
/// brief half): a mem that predates its binding is onboarding, expected-0%, with
/// the concrete backfill path — never a failure or red verdict.
fn render_adopt_framing(resolved: &ResolvedIngest) -> String {
    format!(
        "## First sync — adopting `{}`\n\n\
         This mem predates its binding: it has no anchors and no prior sync baseline, so \
         **0% anchored is expected — this is onboarding, not a failure.** Do not read it \
         as drift or a red verdict. There is no cursor to diff against, so the baseline is \
         the **current** source HEAD — do **not** replay the whole history; treat the \
         current source state as the starting point, and this is a **first sync**.\n\n\
         **Backfill path:** run `memstead projection verify {}` to enumerate the in-scope \
         source artifacts that carry no entity yet, then cover the clearly-new concepts \
         among them through the normal MCP mutation surface — the same conservative rules \
         below apply. Backfilling is incremental: a partial pass is fine, and the next \
         sync continues where you left off.\n\n",
        resolved.destination_mem, resolved.name
    )
}

/// Render the **stale-claim search** block — the bounded step that closes the
/// slice-blinkering blind spot: a changed fact can be claimed by entities
/// whose anchors lie entirely outside the changed slice, so steering repairs
/// at slice-anchored entities alone leaves those claims standing falsified.
///
/// The shape is deliberately bounded, and the prose binds itself to **the
/// changed facts extracted from the slice**: a cosmetic change (formatting,
/// comments, moves that alter no fact) yields an empty fact set, and an empty
/// fact set instructs nothing — no whole-mem sweep, no live-verify of every
/// entity, no rewrite license. Rendered only when the cursor carries actual
/// changed artifacts (never for reseed-only / no-signal-only passes).
fn render_stale_claim_search(resolved: &ResolvedIngest) -> String {
    format!(
        "## Stale claims beyond the slice — search, then judge\n\n\
         A changed fact can be claimed by an entity whose anchors are all outside the \
         changed slice — anchor-steered repairs alone would leave that claim standing \
         falsified. Extract the **changed facts** from the changed artifacts above: \
         renamed identifiers, changed values or defaults, changed behaviors (e.g. an \
         exit code, a flag's meaning), removed or moved concepts. For each changed \
         fact, search the destination mem `{}` for claims about it (`memstead_search` \
         and its variants — try the new name, the old name/value, and close synonyms), \
         and judge **only** the entities whose claims actually mention a changed fact: \
         repair a claim the change falsifies, leave everything else untouched.\n\n\
         This is a bounded fact-search, not a live-verify of every entity and not a \
         rewrite license. If the changes carry no factual claims (formatting, \
         comments, cosmetic moves), the fact set is empty and this step ends with no \
         search and no edits.\n\n",
        resolved.destination_mem
    )
}

/// Render the sync brief's conservatism block — the whole of `/reconcile`'s
/// absorbed judgment (C3): the five conservatism rules, edge-removal
/// conservatism, and rationale-not-changelog.
fn render_sync_conservatism() -> String {
    let lines: Vec<&str> = vec![
        "## How to repair — be conservative",
        "",
        "Repair only what the source changes and the findings above actually justify:",
        "",
        // The five conservatism rules.
        "- **Unsure whether an entity is affected — skip it.** A missed update is a later \
         finding; a wrong rewrite is damage.",
        "- **Do not create a new entity unless the change clearly introduces a new concept \
         with no existing entity.** Prefer updating the entity that already owns the \
         concept.",
        "- **Do not delete an entity unless the change removes the concept entirely.** \
         Deletions a prune pass surfaces follow prune's own never-clobber / conflict-flag \
         rules — never delete on a hunch here.",
        "- **Never rewrite a section that has not changed** — touch only the part the \
         change or finding actually affects.",
        "- **No speculative edges — add only relationships the diff literally introduces** \
         (a new `use` / `import` / dependency you can point at in the change).",
        // Edge-removal conservatism.
        "- **A dropped dependency FLAGS, it does not auto-remove.** If the change removes an \
         import or dependency, leave the matching edge intact and note it for a later \
         audit — removals are ambiguous (temporary refactor vs. permanent cut), and a \
         stale edge is less damaging than an erased real one. **Edge removal is out of \
         scope for sync.**",
        // Rationale-not-changelog.
        "- **Rationale is reasoning, not a changelog.** When you record why a change was \
         made, append the *reasoning* (why this approach, which trade-offs) — never \
         `[commit <hash>]` log-style entries.",
        "",
    ];

    format!("{}\n", lines.join("\n"))
}

/// Render the **sync brief** (C2/C3) — the *single* channel through which
/// maintenance-writing work reaches an agent.
///
/// One brief carries **both** inputs: the cursor slice (`cursor`, rendered via
/// [`render_changed_slice`], which also carries the first-sync reseed framing and
/// the disposition-recording step) and the open verify findings (`findings`, the
/// store's `current(key)` slice). It absorbs the whole of `/reconcile`'s judgment
/// (C3): the five conservatism rules, edge-removal conservatism,
/// rationale-not-changelog, the commits-nothing / engine-commits-per-mutation
/// posture, and — when `adopt` is set — the first-sync/adopt onboarding framing
/// (E1's brief half). A rule-by-rule absorption map records where each retired
/// reconcile rule now lives (bundle plan `05-verify-sync-engine`, C4).
///
/// A slice that carries actual changed artifacts additionally renders the
/// bounded **stale-claim search** step ([`render_stale_claim_search`]) — the
/// beyond-the-slice fact search that catches claims falsified by the change in
/// entities whose anchors never intersect the slice.
///
/// Prune proposals (group F) ride this same brief — F3's single-writer
/// invariant: every prune removal reaches the mem only via an agent acting on
/// this sync brief. They are rendered as proposals only; nothing is auto-deleted.
///
/// When nothing has moved, no findings are open, no prune proposals exist, and
/// this is not an adopt pass, the brief renders a compact "nothing to sync" note
/// instead of the repair machinery — a valid, silent outcome mirroring the build
/// brief's no-op roam.
pub fn render_sync_brief(
    resolved: &ResolvedIngest,
    cursor: &SourceCursor,
    findings: &[Finding],
    prune: &[PruneProposal],
    adopt: bool,
) -> String {
    let preface = render_changed_slice(cursor);
    let open_findings = render_open_findings(findings);
    let prune_block = render_prune_proposals(prune);
    let has_work =
        adopt || !preface.is_empty() || !open_findings.is_empty() || !prune_block.is_empty();

    let mut parts: Vec<String> = vec![render_sync_situation(resolved)];

    if !has_work {
        parts.push(
            "## Nothing to sync\n\nThe source has not moved since the last sync, no \
             verify findings are open, and no prune proposals stand. There is nothing to \
             repair this pass — reporting \"no changes\" is a valid outcome.\n\n"
                .to_string(),
        );
        return parts
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("");
    }

    if adopt {
        parts.push(render_adopt_framing(resolved));
    }
    parts.push(preface);
    // The stale-claim search rides only a slice that carries actual changed
    // artifacts — its facts are extracted FROM those artifacts, so a pass
    // with no changes (findings-only, reseed-only, prune-only) renders none.
    if cursor.any_changes {
        parts.push(render_stale_claim_search(resolved));
    }
    parts.push(open_findings);
    parts.push(prune_block);
    parts.push(render_sync_conservatism());

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
            mode: BuildMode::Discovery,
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
        r.mode = BuildMode::OneShot;
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
        let anchors = brief.find("## Provenance — anchor your writes").unwrap();
        assert!(
            sit < src && src < goal && goal < op && op < anchors,
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

    fn note(source: &str, reason: NoSignalReason) -> NoSignalNote {
        NoSignalNote {
            source: source.to_string(),
            reason,
        }
    }

    /// No changes and no reseed → the block is empty (brief stays a plain roam).
    #[test]
    fn changed_slice_empty_when_nothing_moved() {
        let cursor = SourceCursor {
            union: slice(&[], &[], &[]),
            write_commands: vec![],
            reseed: vec![],
            no_signal: vec![],
            any_changes: false,
            degraded: false,
            dead_denies: vec![],
            dest_mem: "engine".to_string(),
            binding_id: "engine/graph".to_string(),
        };
        assert_eq!(render_changed_slice(&cursor), "");
    }

    /// A zero-selecting deny entry surfaces as a rendered warning even when
    /// nothing else moved — it is never a silent no-op. The entry name and the
    /// migration hint both appear.
    #[test]
    fn changed_slice_renders_dead_deny_warning() {
        let cursor = SourceCursor {
            union: slice(&[], &[], &[]),
            write_commands: vec![],
            reseed: vec![],
            no_signal: vec![],
            any_changes: false,
            degraded: false,
            dead_denies: vec!["dev".to_string(), "typo/**".to_string()],
            dest_mem: "engine".to_string(),
            binding_id: "engine/graph".to_string(),
        };
        let out = render_changed_slice(&cursor);
        assert!(out.contains("deny_paths` entries match nothing"));
        assert!(out.contains("- `dev`"));
        assert!(out.contains("- `typo/**`"));
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
            no_signal: vec![],
            any_changes: true,
            degraded: false,
            dead_denies: vec![],
            dest_mem: "engine".to_string(),
            binding_id: "engine/graph".to_string(),
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
            "### Recording your dispositions (do this LAST)\n",
            "Only after you have worked the changed artifacts above — and only for the artifacts you actually judged — record a disposition for each, so the next pass targets just what changes next. This advance is resumable and non-stalling: a partial pass is honored, and if the source moves mid-pass the remaining slice re-presents (remaining + new) without losing your recorded work.\n",
            "Anchored work disposes itself: at advance time, every listed artifact that an anchor in the destination mem references is marked `worked` automatically (an explicit disposition you pass wins over the auto-mark). Supply dispositions only for the residue — artifacts you skipped, judged out of intent, or worked without anchors. The gate accepts only artifact ids listed above — an unknown id refuses the whole call. When every artifact is disposed, the sync baseline advances automatically. Run:\n",
            "```sh",
            r#"memstead projection advance engine/graph --dispositions '{"<artifact>": "<disposition>", ...}'"#,
            "```",
            "If you were interrupted before finishing, that is fine — your recorded dispositions persist, and the next run re-presents only what is left.\n",
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
            no_signal: vec![],
            any_changes: false,
            degraded: false,
            dead_denies: vec![],
            dest_mem: "d".to_string(),
            binding_id: "d/p".to_string(),
        };
        let out = render_changed_slice(&cursor);
        assert!(out.starts_with("## Source changes since the last sync\n\n"));
        assert!(out.contains(
            "No prior sync baseline exists for `ing/f` — treating the current source state as the baseline (first sync). No priority slice from it this pass; proceed as usual."
        ));
        assert!(out.contains(
            r#"memstead projection advance d/p --dispositions '{"<artifact>": "<disposition>", ...}'"#
        ));
        assert!(
            !out.contains("The source moved"),
            "no 'moved' copy when only reseeding"
        );
    }

    /// Every no-signal reason renders a distinct, named note under the preface,
    /// distinguishable from one another and from a genuinely-unchanged source
    /// (which renders nothing). With no changes and no reseed there is no
    /// recording block, but the preface is non-empty — a source's blindness is
    /// visible. `signal:none` renders literally.
    #[test]
    fn changed_slice_renders_no_signal_reasons_distinguishably() {
        let cursor = SourceCursor {
            union: slice(&[], &[], &[]),
            write_commands: vec![],
            reseed: vec![],
            no_signal: vec![
                note("code-facet", NoSignalReason::Unscoped),
                note("plan-facet", NoSignalReason::DetectionNone),
                note("git-facet", NoSignalReason::GitUnavailable),
                note("ref-mem", NoSignalReason::GraphSnapshotMissing),
            ],
            any_changes: false,
            degraded: false,
            dead_denies: vec![],
            dest_mem: "d".to_string(),
            binding_id: "d/p".to_string(),
        };
        let out = render_changed_slice(&cursor);
        assert!(out.starts_with("## Source changes since the last sync\n"));
        assert!(out.contains("Some sources produced **no change signal**"));
        // Each source is named and carries its own distinct reason text.
        assert!(out.contains("- `code-facet`: unscoped facet (no allow patterns)"));
        assert!(
            out.contains("- `plan-facet`: `signal:none`"),
            "detection-none renders the literal signal:none state"
        );
        assert!(out.contains("- `git-facet`: git signal unavailable"));
        assert!(out.contains("- `ref-mem`: graph snapshot missing"));
        // The four reason texts are mutually distinct.
        let texts = [
            no_signal_reason_text(NoSignalReason::Unscoped),
            no_signal_reason_text(NoSignalReason::DetectionNone),
            no_signal_reason_text(NoSignalReason::GitUnavailable),
            no_signal_reason_text(NoSignalReason::GraphSnapshotMissing),
        ];
        for (i, a) in texts.iter().enumerate() {
            for b in &texts[i + 1..] {
                assert_ne!(a, b, "each no-signal reason must render distinctly");
            }
        }
        // No baseline to advance → no recording block, no "moved" copy.
        assert!(!out.contains("### Recording your dispositions"));
        assert!(!out.contains("The source moved"));
    }

    /// A changed source and a no-signal source coexist: the changed slice AND
    /// the no-signal note both render in the one preface, and the changed
    /// source still emits its recording command.
    #[test]
    fn changed_slice_mixes_changes_and_no_signal() {
        let cursor = SourceCursor {
            union: slice(&[], &["b.rs"], &[]),
            write_commands: vec![cmd("ing/f", "HEAD")],
            reseed: vec![],
            no_signal: vec![note("other", NoSignalReason::Unscoped)],
            any_changes: true,
            degraded: false,
            dead_denies: vec![],
            dest_mem: "d".to_string(),
            binding_id: "d/p".to_string(),
        };
        let out = render_changed_slice(&cursor);
        assert!(out.contains("The source moved"));
        assert!(out.contains("**Modified:**"));
        assert!(out.contains("- `other`: unscoped facet"));
        assert!(out.contains("### Recording your dispositions"));
        assert!(out.contains(
            r#"memstead projection advance d/p --dispositions '{"<artifact>": "<disposition>", ...}'"#
        ));
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
        r.mode = BuildMode::OneShot;
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
            brief.contains("## Provenance — anchor your writes"),
            "one-shot carries the anchor instruction"
        );
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
            no_signal: vec![],
            any_changes: true,
            degraded: true,
            dead_denies: vec![],
            dest_mem: "d".to_string(),
            binding_id: "d/p".to_string(),
        };
        let out = render_changed_slice(&cursor);
        assert!(out.contains(&format!("- …and {} more added", 3)));
        assert!(out.contains("Precise change history for one or more facets was unavailable"));
        // The brief renders the `projection advance` line (the token is no longer
        // an operator command — the engine computes and records it, D4/D7).
        assert!(out.contains(
            r#"memstead projection advance d/p --dispositions '{"<artifact>": "<disposition>", ...}'"#
        ));
    }

    // ---- verify + sync briefs (group C) ----------------------------------

    fn finding(class: FindingClass, target: FindingTarget, detail: &str) -> Finding {
        Finding {
            key: crate::ingest::findings::FindingKey {
                binding_hash: "h".to_string(),
                source_head: "s".to_string(),
            },
            facet: "src".to_string(),
            target,
            class,
            detail: detail.to_string(),
            created_at: "1".to_string(),
        }
    }

    fn anchor_target(entity: &str, artifact: &str) -> FindingTarget {
        FindingTarget::Anchor {
            entity: entity.to_string(),
            artifact: artifact.to_string(),
        }
    }

    fn artifact_target(artifact: &str) -> FindingTarget {
        FindingTarget::Artifact {
            artifact: artifact.to_string(),
        }
    }

    fn empty_cursor() -> SourceCursor {
        SourceCursor {
            union: slice(&[], &[], &[]),
            write_commands: vec![],
            reseed: vec![],
            no_signal: vec![],
            any_changes: false,
            degraded: false,
            dead_denies: vec![],
            dest_mem: "engine".to_string(),
            binding_id: "engine/graph".to_string(),
        }
    }

    /// C1 — the verify brief measures + adjudicates, and carries NO
    /// destination-mutation instruction of any kind. It names the sync brief as
    /// the repair home and prints its explicit no-mutation refusal.
    #[test]
    fn verify_brief_measures_and_refuses_mutation() {
        let r = resolved("engine", None, vec![]);
        let out = render_verify_brief(&r, 3);
        // Measurement + capped adjudication instructions.
        assert!(out.starts_with("## Verify — measure fidelity, do not mutate"));
        assert!(out.contains("3 finding(s) are queued for adjudication"));
        assert!(out.contains("per-run adjudication cap"));
        assert!(out.contains("this is a measurement, not a repair"));
        // C1 REFUSAL: structurally no destination-mutation instruction. The
        // brief never tells the agent to write into the mem — it says the
        // opposite, and hands repairs to the sync brief.
        assert!(out.contains("Verify writes **nothing** into the destination mem"));
        assert!(out.contains("memstead projection brief --sync"));
        // No create/update/relate/delete *instruction* — the only occurrences of
        // those verbs are in the negated "do not …" refusal line.
        assert!(out.contains("do not create or delete an entity"));
        assert!(!out.contains("via `memstead_create`"));
        assert!(!out.contains("Run `memstead_update`"));

        // Backlog 0 → the spot-check phrasing, still no mutation instruction.
        let zero = render_verify_brief(&r, 0);
        assert!(zero.contains("No findings are queued for adjudication"));
        assert!(zero.contains("record any drift you observe as a finding"));
        assert!(zero.contains("Verify writes **nothing**"));
    }

    /// C2 — the sync brief carries BOTH inputs in ONE render: the cursor slice
    /// (the changed artifacts) AND the open findings (`current(key)`), plus the
    /// commits-nothing posture.
    #[test]
    fn sync_brief_carries_both_cursor_and_findings() {
        let r = resolved("engine", None, vec![]);
        let cursor = SourceCursor {
            union: slice(&["gone.rs"], &["moved.rs"], &[]),
            write_commands: vec![cmd("engine/graph/src#synced", "HEAD")],
            reseed: vec![],
            no_signal: vec![],
            any_changes: true,
            degraded: false,
            dead_denies: vec![],
            dest_mem: "engine".to_string(),
            binding_id: "engine/graph".to_string(),
        };
        let findings = vec![
            finding(
                FindingClass::Drifted,
                anchor_target("engine--e", "src/moved.rs"),
                "prepared-content hash drifted",
            ),
            finding(
                FindingClass::Uncovered,
                artifact_target("src/new.rs"),
                "in scope, no anchor",
            ),
        ];
        let out = render_sync_brief(&r, &cursor, &findings, &[], false);
        // Both inputs present in one brief (C2).
        assert!(out.contains("## Source changes since the last sync"));
        assert!(out.contains("`moved.rs`"));
        assert!(out.contains("## Open findings to repair"));
        assert!(out.contains("`engine--e` → `src/moved.rs`"));
        assert!(out.contains("`src/new.rs`"));
        // Sole-writer + commits-nothing posture (C3).
        assert!(out.contains("sole maintenance writer"));
        assert!(out.contains("commits each one **per-mutation**"));
        assert!(out.contains("Sync commits nothing."));
    }

    /// C3 — the sync brief carries the whole absorbed reconcile judgment: the
    /// five conservatism rules, edge-removal conservatism, and
    /// rationale-not-changelog. Each rule is quoted verbatim so absorption is
    /// verifiable against the C4 diff artifact.
    #[test]
    fn sync_brief_absorbs_reconcile_conservatism() {
        let r = resolved("engine", None, vec![]);
        let findings = vec![finding(
            FindingClass::Uncovered,
            artifact_target("src/x.rs"),
            "d",
        )];
        let out = render_sync_brief(&r, &empty_cursor(), &findings, &[], false);
        // Five conservatism rules.
        assert!(out.contains("Unsure whether an entity is affected — skip it."));
        assert!(out.contains(
            "Do not create a new entity unless the change clearly introduces a new concept"
        ));
        assert!(
            out.contains("Do not delete an entity unless the change removes the concept entirely.")
        );
        assert!(out.contains("Never rewrite a section that has not changed"));
        assert!(out.contains(
            "No speculative edges — add only relationships the diff literally introduces"
        ));
        // Edge-removal conservatism — flags, never auto-removes.
        assert!(out.contains("A dropped dependency FLAGS, it does not auto-remove."));
        assert!(out.contains("Edge removal is out of scope for sync."));
        // Rationale-not-changelog.
        assert!(out.contains("Rationale is reasoning, not a changelog."));
        assert!(out.contains("`[commit <hash>]` log-style entries"));
    }

    /// C3 — the first-sync/adopt framing (E1's brief half): a mem predating its
    /// binding is onboarding, expected-0%, with the backfill path — never a
    /// failure. The changed-slice reseed carries the per-facet first-sync note.
    #[test]
    fn sync_brief_renders_adopt_framing() {
        let mut r = resolved("engine", None, vec![]);
        // In a real ResolvedIngest, `name` is the canonical binding id
        // `<mem>/<stem>` while `destination_mem` is the mem — the header uses the
        // mem, the backfill command uses the binding id.
        r.name = "engine/graph".to_string();
        let out = render_sync_brief(&r, &empty_cursor(), &[], &[], true);
        assert!(out.contains("## First sync — adopting `engine`"));
        assert!(out.contains("0% anchored is expected — this is onboarding, not a failure."));
        assert!(out.contains("do **not** replay the whole history"));
        assert!(out.contains("**Backfill path:**"));
        assert!(out.contains("memstead projection verify engine/graph"));
    }

    /// The reseed (first-sync, no cursor) framing lives in the embedded
    /// changed-slice preface — the sync brief inherits it for free.
    #[test]
    fn sync_brief_inherits_first_sync_reseed_framing() {
        let r = resolved("engine", None, vec![]);
        let mut cursor = empty_cursor();
        cursor.reseed = vec![cmd("engine/graph/src#synced", "TOK")];
        let out = render_sync_brief(&r, &cursor, &[], &[], false);
        assert!(out.contains("No prior sync baseline exists for"));
        assert!(out.contains("(first sync)"));
    }

    /// A no-work sync pass (nothing moved, no findings, not adopt) renders a
    /// compact "nothing to sync" note and no repair machinery — a valid outcome.
    #[test]
    fn sync_brief_nothing_to_sync() {
        let r = resolved("engine", None, vec![]);
        let out = render_sync_brief(&r, &empty_cursor(), &[], &[], false);
        assert!(out.contains("## Nothing to sync"));
        assert!(!out.contains("## How to repair"));
        assert!(!out.contains("## Open findings"));
    }

    /// C2 REFUSAL complement — the sync brief is the ONLY render carrying repair
    /// instructions; the verify brief carries none. The verify brief has no
    /// "## How to repair" / "## Open findings to repair" block; the sync brief
    /// has both.
    #[test]
    fn only_sync_brief_carries_repair_instructions() {
        let r = resolved("engine", None, vec![]);
        let findings = vec![finding(
            FindingClass::Drifted,
            anchor_target("engine--e", "src/a.rs"),
            "d",
        )];
        let verify = render_verify_brief(&r, 1);
        let sync = render_sync_brief(&r, &empty_cursor(), &findings, &[], false);
        // Verify: no repair section, no repair verbs as instructions.
        assert!(!verify.contains("## How to repair"));
        assert!(!verify.contains("Update the affected section"));
        // Sync: both repair sections present.
        assert!(sync.contains("## How to repair — be conservative"));
        assert!(sync.contains("## Open findings to repair"));
        assert!(sync.contains("Update the affected section to match"));
    }

    /// Criterion — a changed slice renders the bounded **stale-claim search**
    /// step: extract the changed facts, search the destination mem for claims
    /// about them, judge only entities whose claims mention a changed fact.
    #[test]
    fn sync_brief_changed_slice_renders_stale_claim_search() {
        let r = resolved("engine", None, vec![]);
        let cursor = SourceCursor {
            union: slice(&[], &["moved.rs"], &[]),
            write_commands: vec![cmd("engine/graph/src#synced", "HEAD")],
            reseed: vec![],
            no_signal: vec![],
            any_changes: true,
            degraded: false,
            dead_denies: vec![],
            dest_mem: "engine".to_string(),
            binding_id: "engine/graph".to_string(),
        };
        let out = render_sync_brief(&r, &cursor, &[], &[], false);
        assert!(out.contains("## Stale claims beyond the slice — search, then judge"));
        // The search is bound to the changed facts and the destination mem.
        assert!(out.contains("Extract the **changed facts** from the changed artifacts above"));
        assert!(out.contains("search the destination mem `engine`"));
        assert!(out.contains("`memstead_search`"));
        assert!(out.contains("judge **only** the entities whose claims actually mention"));
        // Bounded shape, spelled out: not a live-verify, not a rewrite license,
        // and an empty fact set (cosmetic change) instructs nothing.
        assert!(out.contains("not a live-verify of every entity"));
        assert!(out.contains("not a rewrite license"));
        assert!(out.contains("the fact set is empty and this step ends with no"));
        // REFUSAL complement: the never-rewrite-unchanged-sections rule still
        // rides the same brief — idempotence stays protected.
        assert!(out.contains("Never rewrite a section that has not changed"));
    }

    /// REFUSAL — the stale-claim search is absent from every pass whose cursor
    /// carries no changed artifacts: findings-only, reseed-only (first sync),
    /// and nothing-to-sync briefs instruct no fact search and no mem sweep.
    #[test]
    fn sync_brief_without_changes_renders_no_stale_claim_search() {
        let r = resolved("engine", None, vec![]);
        let heading = "## Stale claims beyond the slice";

        // Findings-only pass (source unmoved).
        let findings = vec![finding(
            FindingClass::Uncovered,
            artifact_target("src/x.rs"),
            "d",
        )];
        let out = render_sync_brief(&r, &empty_cursor(), &findings, &[], false);
        assert!(!out.contains(heading), "findings-only pass must not search");

        // Reseed-only pass (first sync, no diffable slice).
        let mut reseed_cursor = empty_cursor();
        reseed_cursor.reseed = vec![cmd("engine/graph/src#synced", "TOK")];
        let out = render_sync_brief(&r, &reseed_cursor, &[], &[], false);
        assert!(!out.contains(heading), "reseed-only pass must not search");

        // Nothing-to-sync pass.
        let out = render_sync_brief(&r, &empty_cursor(), &[], &[], false);
        assert!(!out.contains(heading));
    }

    /// A large findings group caps at FINDINGS_CAP with an overflow line —
    /// mirroring the changed-slice cap, so no facet renders unbounded.
    #[test]
    fn sync_brief_caps_large_findings_group() {
        let r = resolved("engine", None, vec![]);
        let findings: Vec<Finding> = (0..FINDINGS_CAP + 4)
            .map(|i| {
                finding(
                    FindingClass::Uncovered,
                    artifact_target(&format!("src/f{i}.rs")),
                    "d",
                )
            })
            .collect();
        let out = render_sync_brief(&r, &empty_cursor(), &findings, &[], false);
        assert!(out.contains("- …and 4 more"));
        // The last few beyond the cap are not rendered inline.
        assert!(!out.contains(&format!("src/f{}.rs", FINDINGS_CAP + 3)));
    }
}
