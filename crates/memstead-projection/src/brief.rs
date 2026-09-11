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
use super::slice::{NoSignalReason, Slice};
use memstead_base::binding::BuildMode;
use memstead_base::binding_run::{ResolvedIngest, ResolvedSource};
use memstead_base::pipeline::{MediumType, PatternMode};

/// Per-class cap on the rendered changed slice — mirrors the plugin's
/// `SLICE_CAP`. Beyond it a `…and N more` line stands in.
const SLICE_CAP: usize = 25;

/// The schema every `ingest/<name>` process mem pins. (The historical
/// plugin-side twin of this constant is retired — this is the single
/// authority.)
pub const PROCESS_MEM_SCHEMA: &str = "ingest@0.5.0";

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
    destination_note: Option<&str>,
    absent_sources: &[String],
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
                    // Name first, medium type demoted to annotation — the
                    // provenance section instructs `source` = the declared
                    // NAME, so this section must teach the same token
                    // (plan 03a: an agent copying this bullet verbatim
                    // must not earn an INVALID_ANCHOR).
                    lines.push(format!(
                        "- **{}** ({}, primary) — `{}`",
                        p.name,
                        medium_type_label(p.medium_type),
                        p.pointer
                    ));
                    // Same obligation as the destination note: an agent
                    // told to read a tree that is not there has been sent
                    // on work it cannot do, and cannot tell that from a
                    // source that is merely empty.
                    if absent_sources.iter().any(|n| n == &p.name) {
                        lines.push(
                            "  - **This source does not resolve to anything on disk.** \
                             Nothing can be read from it until the path exists or the \
                             binding's pointer is corrected."
                                .to_string(),
                        );
                    }
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
                    // Scope is medium-shaped, and so is the label. A graph
                    // source selects entities, so calling its selectors
                    // "Paths" sent the agent looking for a glob tool over a
                    // mem — which does not exist. The changed slice alone is
                    // a delta with no baseline; the reference-mem block below
                    // is the precedent for directing an agent at a mem's
                    // contents without dumping them, so a primary graph
                    // source gets the same executable instruction.
                    let is_graph = p.medium_type == MediumType::Graph;
                    let (allow_label, deny_label) = if is_graph {
                        ("Entities", "Excluding")
                    } else {
                        ("Paths", "Ignore")
                    };
                    if !allows.is_empty() {
                        lines.push(format!("  - {allow_label}: {}", allows.join(", ")));
                    }
                    if !denies.is_empty() {
                        lines.push(format!("  - {deny_label}: {}", denies.join(", ")));
                    }
                    // A scope pattern still in the retired workspace-relative
                    // dialect selects nothing under the pointer join. The
                    // brief is the one surface a binding running only build
                    // and sync ever reads, so the warning must land HERE —
                    // the verify report and the `--full` refusal reach only
                    // bindings that verify.
                    for note in super::cursor::scope_migration_notes(p) {
                        let rewrite = match &note.suggested {
                            Some(s) => format!(" — rewrite it as `{s}`"),
                            None => String::new(),
                        };
                        lines.push(format!(
                            "  - **Scope pattern `{}` is written against the workspace root \
                             rather than the source pointer, so it selects nothing**{rewrite}.",
                            note.pattern
                        ));
                    }
                    if is_graph {
                        lines.push(format!(
                            "  - Read the source baseline with `memstead_search mem={}` \
                             (add `entity_type=` to match a `type:` selector). The changed \
                             slice below is a delta against the last pass — it is not the \
                             whole source, and an entity absent from it may still be \
                             unprojected.",
                            p.pointer
                        ));
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
    // No pinned schema means the engine could not resolve the destination as
    // a mem of this workspace — a binding scaffolded before its mem exists,
    // which `projection init` deliberately allows. Say so here rather than
    // describing a destination that is not there: the brief's mandate is to
    // mutate this mem, and an agent that discovers its absence on the first
    // create has been told something untrue by the surface that sent it.
    // The caller supplies this: whether the destination resolves, and what
    // to do about it, both depend on the workspace shape — which this
    // renderer cannot see. A remedy naming a command that refuses in the
    // reader's own workspace is the defect this note exists to prevent.
    if let Some(note) = destination_note {
        lines.push(format!("  - {note}"));
    }
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
    /// The sync-state key, `"<binding-id>/<facet>#synced"`.
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
    /// The source's medium, when it is a primary source. Carried so the
    /// remedy the note prints is one this medium actually accepts — a
    /// medium-agnostic remedy told a graph source's agent to write `**/*`,
    /// which the engine then refuses as not an entity selector. `None` for a
    /// reference mem, which has no facet scope to remedy.
    pub medium_type: Option<MediumType>,
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
    /// hard error: the ingest still runs, the entry just does nothing. The
    /// scaffold's own default hygiene entries are exempt at collection
    /// (`cursor::dead_deny_entries`) — the engine never calls its own output
    /// a typo.
    pub dead_denies: Vec<String>,
    /// The destination mem whose `sync_state` the baseline tokens live on.
    pub dest_mem: String,
    /// The canonical binding id `<mem>/<stem>` — rendered into the
    /// `memstead projection advance <binding-id> …` line the changed-slice
    /// preface now emits instead of a raw `mem set-sync-state` command (D4/D7).
    pub binding_id: String,
    /// Touchpoint B: one ordered delivery sequence per primary source that
    /// declares a delivery preparation. Their unit ids also ride `union`
    /// (the advance gate accepts them); the class lists never repeat them,
    /// because a class list is alphabetical and a sequence is not.
    pub delivery: Vec<DeliverySequence>,
}

/// One unit of a [`DeliverySequence`], as presented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveredUnit {
    /// The unit's artifact id, `<path>#<key>` ([`memstead_base::preparation::unit_id`]).
    pub id: String,
    /// The unit's intrinsic order key; the sequence sorts by it, then by id.
    pub order_key: String,
    /// How the unit changed (every unit of a first delivery is `Added`).
    pub change: memstead_base::preparation::UnitChange,
    /// Already disposed in the binding's in-progress advance store, so it is
    /// counted but not re-presented.
    pub disposed: bool,
}

/// The ordered delivery sequence of one source under a delivery preparation
/// (touchpoint B of [`memstead_base::preparation`]): the same source state yields the
/// same sequence on every pass, first run and change run alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliverySequence {
    /// The source's declared name.
    pub source: String,
    /// The declared delivery preparation identifier.
    pub preparation: String,
    /// No baseline existed: every unit of the source is delivered, in order.
    pub first_run: bool,
    /// For at least one changed file no baseline content was retrievable,
    /// so every unit of that file is listed rather than only the changed ones.
    pub degraded: bool,
    /// How many not-yet-disposed units to present this pass (the build
    /// operation's `batch_size`; `0` presents all).
    pub batch: usize,
    /// Every delivered unit, in the total order.
    pub units: Vec<DeliveredUnit>,
}

/// How a mention-steered entity names a changed artifact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MentionKind {
    /// The body names the artifact's path (workspace- or source-relative).
    Path,
    /// The body names, in an inline code span, a symbol the change's diff
    /// defines or removes.
    Symbol(String),
}

/// One destination entity steered at a changed artifact by mention, not by
/// anchor: it names the artifact (or a symbol the change defines or removes)
/// in its prose and carries no anchor on the artifact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MentionRow {
    /// The entity id (`mem--slug`).
    pub entity: String,
    /// What the body names.
    pub kind: MentionKind,
    /// The section the mention was found in.
    pub section: String,
}

/// The destination entities one changed artifact steers: the ones anchoring
/// it, and the ones naming it without an anchor. An entity that anchors the
/// artifact is listed once, under anchors, never under mentions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactEntities {
    /// Entity ids whose anchors reference the artifact, sorted.
    pub anchored: Vec<String>,
    /// Mention-steered rows, sorted by entity then kind.
    pub mentioned: Vec<MentionRow>,
}

/// Per changed artifact (workspace-relative id, the spelling the slice
/// lists), the entities it steers. Built from entity bodies at brief time;
/// nothing is stored in the mem. Artifacts steering no entity are absent.
pub type SteeredEntities = std::collections::BTreeMap<String, ArtifactEntities>;

/// The include key that forces the mention lines past the brief budget.
pub const BRIEF_INCLUDE_MENTIONS: &str = "mentions";

/// The default token budget for the brief's heavy content (the mention
/// lines): the house envelope budget the fidelity report also uses.
pub const DEFAULT_BRIEF_BUDGET: usize = super::report::DEFAULT_REPORT_BUDGET;

/// Single-quote a value for the emitted shell command, escaping embedded
/// single quotes. The digest token is JSON (contains `"` and `:`), so it
/// must be quoted to survive the shell. Mirrors the plugin's `shellQuote`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Render one changed-slice class (Deleted / Modified / Added), capped at
/// [`SLICE_CAP`] with a `…and N more` overflow line.
/// Render one delivery sequence: the not-yet-disposed units in the total
/// order, numbered by their position in that order (so a unit keeps its
/// number across the passes of one delivery while earlier units are
/// disposed), capped at the sequence's batch with the remainder counted,
/// never reshuffled.
fn render_delivery_sequence(lines: &mut Vec<String>, seq: &DeliverySequence) {
    use memstead_base::preparation::UnitChange;
    lines.push(format!(
        "### Delivery sequence: `{}` (`{}`)\n",
        seq.source, seq.preparation
    ));
    let opening = if seq.first_run {
        "First delivery of this source: every unit, in the source's own order."
    } else {
        "The units that changed since the last pass, at their positions in the source's own \
         order."
    };
    lines.push(format!(
        "{opening} Work them top to bottom: the order derives from the units' own keys, never \
         from discovery or directory order, it is identical on every pass, and a unit assumes \
         only the units numbered before it. Address a unit as `<path>#<key>` in anchors and \
         dispositions.\n"
    ));
    if seq.degraded {
        lines.push(
            "_(No baseline content was retrievable for one or more changed files, so every unit \
             of those files is listed; precision is coarser this pass only.)_\n"
                .to_string(),
        );
    }
    let pending: Vec<(usize, &DeliveredUnit)> = seq
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| !u.disposed)
        .collect();
    let disposed = seq.units.len() - pending.len();
    let shown = if seq.batch == 0 {
        pending.len()
    } else {
        pending.len().min(seq.batch)
    };
    for (position, unit) in &pending[..shown] {
        let label = match unit.change {
            UnitChange::Added => "new",
            UnitChange::Modified => "changed",
            UnitChange::Deleted => "deleted",
        };
        lines.push(format!("{}. `{}` ({label})", position + 1, unit.id));
    }
    if pending.len() > shown {
        lines.push(format!(
            "- …and {} more, presented in order once these are disposed",
            pending.len() - shown
        ));
    }
    if disposed > 0 {
        lines.push(format!(
            "_({disposed} unit{} of this sequence already disposed this pass.)_",
            if disposed == 1 { "" } else { "s" }
        ));
    }
    if pending.is_empty() {
        lines.push(
            "_(Every unit of this sequence is disposed; the baseline advances when the pass \
             completes.)_"
                .to_string(),
        );
    }
    lines.push(String::new());
}

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
fn no_signal_reason_text(reason: NoSignalReason, medium: Option<MediumType>) -> &'static str {
    match reason {
        // The remedy is medium-shaped, because scope is. Naming a path glob at
        // a graph source sent the agent to write the one thing the engine
        // refuses — the brief instructing a write it would then reject.
        NoSignalReason::Unscoped => match medium {
            Some(MediumType::Graph) => {
                "unscoped facet (no allow patterns) — nothing is monitored; write `*` in the \
                 facet scope to watch the whole mem, or `type:<entity_type>` / `id:<glob>` \
                 to narrow it (a graph source selects entities, not paths)"
            }
            _ => {
                "unscoped facet (no allow patterns) — nothing is monitored; write `**/*` in the \
                 facet scope to watch the whole medium"
            }
        },
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
    render_changed_slice_with(cursor, None, DEFAULT_BRIEF_BUDGET, &[])
}

/// Render the entities each changed artifact steers, after the slice
/// classes: the anchoring entities first, then — headed so the agent knows
/// they are steered by mention, not by anchor — the entities naming the
/// artifact's path or a symbol its change defines or removes. The mention
/// lines are the block's heavy content: they greedy-fill under `budget`
/// (tokens) and degrade to a count plus the `--include mentions` hint when
/// they do not fit, never to silence; the anchored lines always ship. An
/// artifact steering nothing is absent; a slice steering nothing renders no
/// block at all.
fn render_steered_entities(
    lines: &mut Vec<String>,
    steered: &SteeredEntities,
    budget: usize,
    include: &[String],
) {
    if steered.is_empty() {
        return;
    }
    let row_text = |r: &MentionRow| -> String {
        let how = match &r.kind {
            MentionKind::Path => "path".to_string(),
            MentionKind::Symbol(s) => format!("`{s}`"),
        };
        format!("    - `{}` ({how}, in `{}`)", r.entity, r.section)
    };
    let mention_entities: std::collections::BTreeSet<&str> = steered
        .values()
        .flat_map(|e| e.mentioned.iter().map(|r| r.entity.as_str()))
        .collect();
    let mention_artifacts = steered.values().filter(|e| !e.mentioned.is_empty()).count();
    let mention_text: String = steered
        .values()
        .flat_map(|e| e.mentioned.iter().map(row_text))
        .collect::<Vec<_>>()
        .join("\n");
    let mention_cost = memstead_base::chunking::estimate_tokens(&mention_text);
    let show_mentions = mention_entities.is_empty()
        || mention_cost <= budget
        || include.iter().any(|k| k == BRIEF_INCLUDE_MENTIONS);

    lines.push("### Entities to walk for each changed artifact\n".to_string());
    lines.push(
        "For every changed artifact, the entities that anchor it come first. The entities \
         under **mentioned by** name the artifact's path, or a symbol the change defines or \
         removes, in their prose without anchoring it: they are steered by mention, not by \
         anchor, and the claim walk over them is part of this pass exactly as for the \
         anchored ones. Read the named section against the artifact, repair what the change \
         falsifies, and anchor the claim while you are there. An entity that anchors the \
         artifact is listed once, under anchors. An artifact with mention-steered entities is \
         not auto-disposed by its anchors: record its disposition yourself after walking \
         them.\n"
            .to_string(),
    );
    for (artifact, e) in steered {
        if e.anchored.is_empty() && !show_mentions {
            continue;
        }
        lines.push(format!("- `{artifact}`"));
        if !e.anchored.is_empty() {
            let ids: Vec<String> = e.anchored.iter().map(|id| format!("`{id}`")).collect();
            lines.push(format!("  - anchored by: {}", ids.join(", ")));
        }
        if show_mentions && !e.mentioned.is_empty() {
            lines.push("  - mentioned by:".to_string());
            lines.extend(e.mentioned.iter().map(row_text));
        }
    }
    if !show_mentions {
        lines.push(format!(
            "- _{} mention-steered entit{} over {} changed artifact{} not listed under the \
             token budget ({mention_cost} tokens; budget {budget}): re-render with \
             `--include {BRIEF_INCLUDE_MENTIONS}` to list them. They are still part of this \
             pass, and their artifacts still need a disposition._",
            mention_entities.len(),
            if mention_entities.len() == 1 {
                "y"
            } else {
                "ies"
            },
            mention_artifacts,
            if mention_artifacts == 1 { "" } else { "s" },
        ));
    }
    lines.push(String::new());
}

/// [`render_changed_slice`] with the entities each changed artifact steers
/// (`steered`, from [`super::cursor::steered_entities`]) listed after the
/// slice classes under `budget` / `include`; `None` renders the classes
/// alone (the build brief's preface).
pub fn render_changed_slice_with(
    cursor: &SourceCursor,
    steered: Option<&SteeredEntities>,
    budget: usize,
    include: &[String],
) -> String {
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
        // Delivery sequences first: for a source under a delivery
        // preparation the order IS the steering, and their unit ids never
        // repeat in the alphabetical class lists below.
        for seq in &cursor.delivery {
            render_delivery_sequence(&mut lines, seq);
        }
        let unit_ids: std::collections::BTreeSet<&str> = cursor
            .delivery
            .iter()
            .flat_map(|s| s.units.iter().map(|u| u.id.as_str()))
            .collect();
        let without_units = |v: &[String]| -> Vec<String> {
            v.iter()
                .filter(|p| !unit_ids.contains(p.as_str()))
                .cloned()
                .collect()
        };
        // Deletions first — cheapest, highest-signal drift.
        render_slice_class(&mut lines, "Deleted", &without_units(&cursor.union.deleted));
        render_slice_class(
            &mut lines,
            "Modified",
            &without_units(&cursor.union.modified),
        );
        render_slice_class(&mut lines, "Added", &without_units(&cursor.union.added));
        if cursor.degraded {
            lines.push(
                "_(Precise change history for one or more facets was unavailable, so its full \
                 current file set is listed above. Detection still fired from the durable baseline; \
                 targeting is coarser this pass only.)_\n"
                    .to_string(),
            );
        }
        if let Some(steered) = steered {
            render_steered_entities(&mut lines, steered, budget, include);
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
            "No usable sync baseline exists for {keys} — none was recorded, or the recorded one \
             is not a commit of the source's repo (foreign or garbage-collected). Treating the \
             current source state as the baseline. No priority slice from {it} this pass; \
             proceed as usual.\n"
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
                no_signal_reason_text(note.reason, note.medium_type)
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
    // resumable and non-stalling: a partial pass is honored on disk, and a
    // source that moves mid-pass re-presents (remaining + new) without losing
    // recorded work. The agent runs `projection advance`, which computes and
    // records the new baseline token engine-side — the brief no longer renders a
    // raw `mem set-sync-state` command. The block appears whenever there is
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
             explicit disposition you pass wins over the auto-mark), except an artifact with \
             mention-steered entities, which waits for your disposition. Supply dispositions \
             only for the residue — artifacts you skipped, judged out of intent, worked without \
             anchors, or walked by mention. The gate accepts only artifact ids listed above — \
             an unknown id refuses the whole call. When every artifact is disposed, the sync \
             baseline advances automatically. Run:\n"
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
pub fn render_anchor_instruction(resolved: &ResolvedIngest) -> String {
    let mut block = "## Provenance — anchor your writes\n\n\
     Attach an `anchors` list to every `memstead_create` / `memstead_update`, naming the \
     source artifact(s) the entity is drawn from (the mutation tools document the element \
     shape). Anchored writes are what verify measures coverage and drift against, and — on \
     cursor-driven passes — what the advance gate auto-marks `worked`; an unanchored write \
     leaves the fidelity report and the disposition window blind to your work.\n\n"
        .to_string();
    // Name the producing entry point: each anchor's `source` carries the
    // binding source NAME it came from, so a discovery run is measurable
    // per entry point (which entry carries, which delivers nothing).
    let primary_names: Vec<&str> = resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            memstead_base::binding_run::ResolvedSource::Primary(src) => Some(src.name.as_str()),
            memstead_base::binding_run::ResolvedSource::Reference { .. } => None,
        })
        .collect();
    if !primary_names.is_empty() {
        block.push_str(&format!(
            "Set each anchor's `source` to the binding source name you drew the artifact \
             from — this binding declares: {}. The name selects the pointer the \
             artifact path is joined onto, so the wrong one usually refuses \
             `INVALID_ANCHOR` (the path resolves under no candidate join). A name \
             outside the list is NOT itself refused when the path happens to \
             resolve workspace-relative — that tolerance exists for anchors whose \
             binding was later renamed — so getting it right is on you, not on a \
             gate.\n\n",
            primary_names
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    // The url grain: the engine never fetches, so the observation is the
    // author's, and so is the stability call — a page that may change reads
    // `unstable` (a hash break is a recheck, not drift); an immutable
    // document must say so to be adjudicated.
    block.push_str(
        "For a web document use `grain: url` with the URL as `artifact` and pass the retrieved \
         text as `content` so the engine records its hash (the engine never fetches). Set \
         `hash_stability: stable` on an IMMUTABLE document — a dated PDF, an archived page, a \
         versioned standard — so a later changed hash reads as `drifted`; leave the default \
         `unstable` for a living page, where a change is only a `recheck`. Url rows are \
         re-adjudicated when someone supplies a fresh observation (`memstead verify-anchors \
         --observations`), and every surface shows how long each has gone unobserved.\n\n",
    );
    // A source under a preparation hashes a PREPARED form, which no agent
    // computes by hand: say so, and say what to do instead.
    for source in &resolved.sources {
        let memstead_base::binding_run::ResolvedSource::Primary(src) = source else {
            continue;
        };
        let Some(prep) = src
            .preparation
            .as_deref()
            .and_then(memstead_base::preparation::lookup)
        else {
            continue;
        };
        let what = match prep.id {
            memstead_base::preparation::CODE_MAP => {
                "the file's interface digest (imports, exports, signatures; comments, \
                 formatting and bodies invisible), and a `tree` anchor the code map of every \
                 scoped file under it"
            }
            memstead_base::preparation::DATED_ENTRIES => {
                "the unit's own text for a `<path>#<key>` span, the file's bytes otherwise"
            }
            memstead_base::preparation::ENTITY_LOAD_BEARING => "the entity's load-bearing sections",
            _ => prep.description,
        };
        block.push_str(&format!(
            "Anchors on `{}` hash a prepared form (`{}`): {what}. Never compute `hash` \
             yourself for this source — leave it empty (verify records it on first \
             observation), or for a `file` or `span` anchor pass the artifact's `content` \
             and the engine hashes the prepared form (a `tree` anchor takes no content).\n\n",
            src.name, prep.id
        ));
    }
    block
}

#[allow(clippy::too_many_arguments)]
pub fn assemble_discovery_brief(
    resolved: &ResolvedIngest,
    intent_findings: &[memstead_base::binding_intent::IntentFinding],
    guidance: &ResolvedGuidance,
    process_mem: &ProcessMemInfo,
    destination_schema: Option<&str>,
    destination_note: Option<&str>,
    absent_sources: &[String],
    changed_slice_preface: &str,
) -> String {
    let parts = [
        render_situation(resolved, process_mem),
        render_intent(resolved),
        memstead_base::binding_intent::render_intent_findings(intent_findings, &resolved.name),
        render_goal_and_avoid(guidance),
        render_operative_data(
            resolved,
            process_mem,
            destination_schema,
            destination_note,
            absent_sources,
        ),
        render_anchor_instruction(resolved),
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
#[allow(clippy::too_many_arguments)]
pub fn assemble_one_shot_brief(
    resolved: &ResolvedIngest,
    intent_findings: &[memstead_base::binding_intent::IntentFinding],
    guidance: &ResolvedGuidance,
    process_mem: &ProcessMemInfo,
    destination_schema: Option<&str>,
    destination_note: Option<&str>,
    absent_sources: &[String],
    destination_purpose: Option<&str>,
) -> String {
    let parts = [
        render_situation(resolved, process_mem),
        render_intent(resolved),
        memstead_base::binding_intent::render_intent_findings(intent_findings, &resolved.name),
        render_goal_and_avoid(guidance),
        render_operative_data(
            resolved,
            process_mem,
            destination_schema,
            destination_note,
            absent_sources,
        ),
        render_anchor_instruction(resolved),
        render_one_shot_lens(resolved, destination_schema, destination_purpose),
    ];
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("")
}

// ---------------------------------------------------------------------------
// Verify + sync briefs — the measure/repair surface beside the build
// briefs. Verify MEASURES (no destination mutation of any kind, C1); sync is the
// SOLE maintenance writer, carrying BOTH the cursor slice and the open findings
// in one brief (C2) with the whole of `/reconcile`'s absorbed judgment (C3). A
// rule-by-rule absorption map records where each retired reconcile rule now
// lives (C4).
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
         you find. **You** write nothing into the destination mem. The run itself records \
         its findings store, which is the verify surface's own state outside the mem, and \
         backfills observed anchor hashes, which is measurement machinery. Its one write \
         into the mem's config, the `#verified` baseline, rides `--advance` and is off by \
         default, so a bare verify leaves that config byte-identical.",
        resolved.name, resolved.destination_mem
    ));
    lines.push(String::new());

    lines.push(
        "Anchors may carry a `source` naming the binding entry point that produced them — \
         note it when recording findings, so fidelity stays measurable per source."
            .to_string(),
    );
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
        "Verify writes **no entity content**. Do not update a \
         `specifies` / `constraints` section, do not create or delete an entity, do not \
         add or remove a relationship. When measurement shows the graph is wrong, that \
         is a **finding** — the sync brief (`memstead projection brief --sync`) is the \
         one place those repairs are made. Leave every fix to it. (The run itself records \
         its findings store, which lives outside the mem, and backfills observed anchor \
         hashes; it writes a `#verified` baseline only under `--advance` — engine \
         bookkeeping, not your edits.)"
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
        FindingTarget::Mention {
            entity,
            artifact,
            section,
        } => format!("`{entity}` names `{artifact}` in `{section}`"),
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
/// when there are no open findings. `binding_id` feeds the buildable
/// `projection exclude` line in the uncovered group's guidance.
fn render_open_findings(findings: &[Finding], binding_id: &str) -> String {
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
         only the part that changed. If the entity is still accurate, leave it. Either \
         way, reset the anchor on the entity in ONE update call: `anchors_unset` the \
         row, then write it fresh in the same call's `anchors` (same artifact, grain, \
         class and source, no hash) — the next verify backfills the freshly observed \
         hash and the drift clears. A hashless re-declare WITHOUT the unset keeps the \
         stored baseline by design and clears nothing, and updating the entity alone, \
         or advancing the baseline, leaves the anchor drifted just the same.",
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
         removals are a prune concern and arrive as proposals in the prune block — \
         do not delete on a hunch here.",
        &group(FindingClass::UnresolvableAnchor),
    );
    // Uncovered — a source artifact with no entity: create only for a clearly-new
    // concept (conservatism rule "no new entities unless clearly-new concept").
    // The third disposition — deliberately not modeled — routes to `projection
    // exclude`, the verb whose gate accepts a stable artifact (`advance` gates on
    // the changed slice, so a stable uncovered artifact is undispositionable
    // there; three campaign runs hit that wall before this line existed).
    let uncovered_guidance = format!(
        "An in-scope source artifact has no anchor in the mem. Create an entity for it \
         **only** if it is a clearly-new concept with no existing entity; otherwise \
         extend the entity that already owns the concept, or leave it for a discovery \
         build. A third answer is legitimate: the artifact is mined and deliberately \
         warrants no entity. Record that with a rationale — it stops presenting here \
         from the next brief on:\n\n```bash\nmemstead projection exclude {binding_id} \
         --exclusions '{{\"<artifact>\": \"<rationale>\"}}'\n```"
    );
    render_findings_group(
        &mut lines,
        "Uncovered — a source artifact with no entity",
        &uncovered_guidance,
        &group(FindingClass::Uncovered),
    );
    // Unanchored mention — a claim about a file the entity does not watch.
    // Verify cannot speak to it until an anchor exists, so the pass reads the
    // claim against the artifact now and then decides its provenance.
    let mention_guidance = format!(
        "The entity's prose names an in-scope source artifact it carries no anchor on, so \
         no verify watches that claim on the entity's behalf. Read the named section against \
         the artifact. If the claim holds, add the anchor in one `memstead_update` call \
         (`anchors`: the artifact, grain `file`, class `anchored` or `informed-by`) so the \
         next verify watches it; if the claim is stale, correct the section and anchor it in \
         the same call. If the artifact is mined and deliberately warrants no entity, record \
         that instead — the mention stops presenting from the next brief on:\n\n```bash\n\
         memstead projection exclude {binding_id} --exclusions '{{\"<artifact>\": \
         \"<rationale>\"}}'\n```"
    );
    render_findings_group(
        &mut lines,
        "Unanchored mention — a claim about an artifact the entity does not anchor",
        &mention_guidance,
        &group(FindingClass::UnanchoredMention),
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

/// Render the authored-exclusion block for the sync brief: what is in force
/// (artifact, source, rationale) and what the reconcile dropped because its
/// source left the declaration. Empty when the ledger is empty.
fn render_exclusions(ledger: &crate::advance::ExclusionLedger) -> String {
    if ledger.active.is_empty() && ledger.dropped.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    if !ledger.active.is_empty() {
        lines.push("## Excluded artifacts (authored)".to_string());
        lines.push(String::new());
        lines.push(
            "These in-scope artifacts are deliberately excluded with a recorded rationale; \
             they never present as uncovered and need no entity. An exclusion keys on the \
             artifact and its source, so it survives edits to the rest of the binding."
                .to_string(),
        );
        lines.push(String::new());
        for e in &ledger.active {
            lines.push(format!(
                "- `{}` (source `{}`): {}",
                e.artifact, e.source, e.rationale
            ));
        }
        lines.push(String::new());
    }
    if !ledger.dropped.is_empty() {
        lines.push("## Exclusions dropped — their source is no longer declared".to_string());
        lines.push(String::new());
        lines.push(
            "The source these exclusions were recorded under left the binding's declaration, \
             so they no longer apply; re-declare the source and record them again if they \
             still hold."
                .to_string(),
        );
        lines.push(String::new());
        for d in &ledger.dropped {
            lines.push(format!(
                "- `{}` (source `{}`, dropped {}): {}",
                d.artifact, d.source, d.dropped_at, d.rationale
            ));
        }
        lines.push(String::new());
    }
    format!("{}\n", lines.join("\n"))
}

/// Render the prune-proposals block for the sync brief (group F) — the deletion
/// proposals prune surfaced, grouped by disposition.
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

    // Proposed — both sides presented, the agent decides. Never an auto-write.
    let proposed = group(PruneDisposition::Proposed);
    if !proposed.is_empty() {
        lines.push("### Proposed removals — the call is yours".to_string());
        lines.push(String::new());
        lines.push(
            "The source no longer holds the artifact(s) these entities describe. Prune states \
             only what it observed; it cannot know whether the entity still earns its place. \
             The rule: **delete** the entity through the mutation surface when its subject is \
             gone from the source and no knowledge mem cites it; **keep** it, with a dated note \
             naming what retired its subject, when one does. Both sides are listed so the call \
             is yours."
                .to_string(),
        );
        lines.push(String::new());
        let shown = proposed.len().min(FINDINGS_CAP);
        for p in &proposed[..shown] {
            lines.push(format!(
                "- `{}` — **source side:** artifact(s) gone: {}; **model side:** the entity is \
                 still present — you decide.",
                p.entity,
                artifact_list(&p.artifacts)
            ));
        }
        if proposed.len() > shown {
            lines.push(format!("- …and {} more", proposed.len() - shown));
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
         Deletions a prune pass surfaces arrive as proposals in the prune block, never as \
         instructions — never delete on a hunch here.",
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
/// (the adopt rule's brief half). A rule-by-rule absorption map records where
/// each retired reconcile rule now lives (C4).
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
    exclusions: &crate::advance::ExclusionLedger,
) -> String {
    render_sync_brief_with(
        resolved,
        cursor,
        findings,
        prune,
        adopt,
        exclusions,
        &SteeredEntities::new(),
        DEFAULT_BRIEF_BUDGET,
        &[],
    )
}

/// [`render_sync_brief`] with the entities the slice steers (`steered`, from
/// [`super::cursor::steered_entities`]) rendered under the changed slice,
/// their mention lines budgeted by `budget` / `include`.
#[allow(clippy::too_many_arguments)]
pub fn render_sync_brief_with(
    resolved: &ResolvedIngest,
    cursor: &SourceCursor,
    findings: &[Finding],
    prune: &[PruneProposal],
    adopt: bool,
    exclusions: &crate::advance::ExclusionLedger,
    steered: &SteeredEntities,
    budget: usize,
    include: &[String],
) -> String {
    let preface = render_changed_slice_with(cursor, Some(steered), budget, include);
    let open_findings = render_open_findings(findings, &resolved.name);
    let prune_block = render_prune_proposals(prune);
    let has_work =
        adopt || !preface.is_empty() || !open_findings.is_empty() || !prune_block.is_empty();

    // The exclusion ledger renders on every sync brief, work or not: an
    // exclusion in force is standing state the agent must not re-litigate,
    // and one the reconcile dropped is reported here, once, with its source.
    let mut parts: Vec<String> = vec![
        render_sync_situation(resolved),
        render_exclusions(exclusions),
    ];

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
    parts.push(render_anchor_instruction(resolved));
    parts.push(render_sync_conservatism());

    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests;
