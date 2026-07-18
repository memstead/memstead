//! `memstead projection` — the binding (projection-promotion) command tree.
//!
//! The projection is the unit: one versioned binding per source→mem obligation
//! (bundle plan `03-projection-promotion`). The tree ships five leaves —
//! `brief`, `init`, `migrate`, `advance`, `enable`:
//!
//! - `brief` renders a binding's run-brief — the Markdown prompt an agent
//!   consumes — for a canonical binding id `<mem>/<stem>` (D3/D9), or the next
//!   due binding under `--all` (round-robin + backoff selection).
//! - `init` scaffolds a fresh v2 single-record binding non-interactively.
//! - `migrate` converts every prior on-disk generation into v2 records in
//!   place: gen-1 root folders, the gen-2 four-primitive store, and the v1
//!   three-file store — folding medium+facet content inline.
//! - `advance` records disposition-gated sync-baseline advances (D7).
//! - `enable` adds a missing `build` / `sync` / `verify` operation block to an
//!   existing binding (D6 — the remedy a refused mutating op cites).
//!
//! This tree is the sole binding surface: the retired `ingest` and `pipeline`
//! command trees folded in here (`ingest brief` → `projection brief`,
//! `pipeline migrate` → `projection migrate`'s gen-1 path).
//!
//! Errors carry `PROJECTION_*` wire tokens (D12); the missing-workspace path is
//! single-sourced through [`crate::setup::workspace_not_initialised_error`].

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use serde_json::json;

use memstead_base::binding::{
    BINDING_VERSION, Binding, BuildMode, BuildOperation, CapabilityError, CoverageSemantics,
    DEFAULT_ADJUDICATION_CAP, DEFAULT_FULL_RESYNC_EVERY, Operations, PruneConfig, SyncOperation,
    VerifyOperation, prune_guarantee_for_medium, validate_binding,
};
use memstead_base::binding_migrate::{
    BindingMigrateError, check_all_consumed, fold_v1_binding, migrate_gen2_bindings,
};
use memstead_base::ingest::advance::{
    AdvanceError, DispositionInput, ExcludeError, advance_baseline, record_exclusions,
};
use memstead_base::ingest::findings::{
    FindingsError, FullResyncDecision, record_anchor_hash_backfill, record_verified_baseline,
    verify_binding, verify_binding_full,
};
use memstead_base::ingest::report::{
    DEFAULT_REPORT_BUDGET, compute_fidelity_report, render_fidelity_report,
};
use memstead_base::ingest::resolve::{ResolveError, ResolvedSource, resolve_binding_run};
use memstead_base::ingest::{
    OperationFilter, OperationKind, RenderBriefError, render_ingest_brief, render_sync_brief_for,
    render_verify_brief_for, select_next_due_operation,
};
use memstead_base::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode, Source};
use memstead_base::pipeline_store::{
    ProjectionGeneration, delete_ingest, load_legacy_pipeline_configs, load_pipeline_configs,
    load_projection_generations, read_binding, remove_mediums_and_facets_trees, write_binding,
};
use memstead_base::workspace_store::StoreError;
use memstead_base::{migrate_legacy_pipeline, read_legacy_pipeline_configs};

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, workspace_not_initialised_error};

#[derive(ClapArgs, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: ProjectionCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProjectionCommand {
    /// Render a binding's run-brief — the Markdown prompt an agent consumes —
    /// on stdout. Takes the canonical binding id `<mem>/<stem>` (D3), e.g.
    /// `engine/graph`. Omit the id (or pass `--all`) to select the next due
    /// (binding, operation) pair by round-robin + backoff and render that
    /// operation's brief; `--operation` picks which operations rotate (default
    /// `build` — the classic build-only rotation; `any` rotates every
    /// loop-declared build / sync / verify pair). An operation participates
    /// only where its binding block declares `trigger: loop`. Reads the v2
    /// binding store and the destination mem's schema / writing guidance; the
    /// assembly is shared with the UniFFI surface, so CLI and app briefs are
    /// byte-identical by construction.
    ///
    /// `--verify` renders the **verify brief** (group C) for the named binding:
    /// measurement + capped-adjudication instructions only, with no
    /// destination-mutation instruction. `--sync` renders the **sync brief** —
    /// the sole maintenance-writer prompt, carrying both the cursor slice and the
    /// open verify findings in one brief with the absorbed reconcile
    /// conservatism. Both are read-only on the mem; the sync brief's repairs
    /// reach the mem only when an agent acts on it through the MCP mutation
    /// surface.
    Brief(BriefArgs),
    /// Scaffold a fresh v2 binding non-interactively: ONE record with one
    /// inline source, at `.memstead/projections/<mem>/<stem>.json`.
    /// All inputs are flags — no prompts ever (parity across callers). The
    /// default binding declares build+sync+verify where the medium permits:
    /// a `web` source scaffolds build-only, with the deferral named in
    /// `warnings[]`. A `prune` block is scaffolded wherever sync survived,
    /// with the strongest guarantee the medium supports (never-clobber for a
    /// git-backed source). Refuses `PROJECTION_EXISTS` (without touching disk)
    /// when a binding of the same id already exists — never overwrites.
    Init(InitArgs),
    /// Migrate every prior on-disk generation into v2 single-record
    /// bindings, in place. Gen-1 — the root-folder
    /// `scopes|projections|ingests/` JSON layout — is first materialized
    /// into the four-primitive store, then folded. Gen-2 — the
    /// four-primitive store (per-mem `Projection` + flat `Ingest`) — merges
    /// each ingest into its projection and folds the referenced facets +
    /// mediums inline. v1 — the three-file store — folds each binding's
    /// facet references inline the same way, source names preserved
    /// byte-verbatim (they key sync watermarks). The emptied `mediums/` and
    /// `facets/` trees are removed; orphan records refuse rather than drop.
    /// `refinement` mode and dangling refs refuse with a typed error.
    /// Idempotent on a migrated store. Use `--dry-run` to preview without
    /// writing.
    Migrate(MigrateArgs),
    /// Enable a `build` / `sync` / `verify` operation on an existing binding by
    /// adding its block (with sensible defaults) if absent. This is the remedy
    /// a refused *mutating* operation cites (D6): `projection enable sync
    /// <binding>`. Before writing, the operation is checked against the
    /// medium-capability matrix (D6) — enabling `sync`/`verify` over a medium
    /// that cannot support it (e.g. a `web` source) refuses with the capability
    /// gap and writes nothing. Enabling an already-present operation refuses
    /// `PROJECTION_OP_ALREADY_ENABLED`; a missing binding refuses
    /// `PROJECTION_NOT_FOUND`.
    Enable(EnableArgs),
    /// Advance a binding's sync baseline by recording per-artifact
    /// dispositions (D7). The engine freezes the presented changed slice,
    /// subtracts already-disposed artifacts on re-presentation, appends
    /// new-HEAD deltas when the source moves mid-pass, and — when the
    /// remainder empties — advances the destination mem's `#synced` token via
    /// the sync-state writer (provenance piggybacks that commit). Dispositions
    /// are durable (`.memstead/state/advance/`), so a partial pass resumes
    /// across process restarts. The gate accepts **only** artifact ids the
    /// engine presented — an unknown id refuses the whole call atomically
    /// (`PROJECTION_ADVANCE_UNKNOWN_ARTIFACT`). In this cycle the agent supplies
    /// a disposition for **every** artifact explicitly (auto-derivation lands
    /// later).
    Advance(AdvanceArgs),
    /// Declare authored **exclusions** for in-scope source artifacts. Unlike
    /// `advance` (whose gate accepts only artifacts in the changed slice), this
    /// gates on enumerable `S(D)` membership, so a stable, unchanged artifact can
    /// be recorded as deliberately not-modeled with a rationale. Each accepted
    /// `(artifact, rationale)` lands in the durable exclusion ledger the fidelity
    /// report consults, so the artifact stops re-surfacing as `uncovered` under
    /// exhaustive coverage and keeps its reasoning. An artifact outside `S(D)`
    /// refuses the whole call atomically (`PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER`);
    /// re-declaring merges into the ledger. The write path for the option-(a)
    /// process-mem judgment migration, and the general "this in-scope artifact is
    /// mined and warrants no destination entity, because …" capability.
    Exclude(ExcludeArgs),
    /// Measure a binding's fidelity and record durable findings (E3b, group A).
    /// Read-only on the destination mem: verify adjudicates the mem's anchors
    /// against the live source and samples in-scope artifacts, writing findings
    /// keyed `(hash(D), source_head)` into the engine-owned findings store
    /// (`.memstead/state/findings/`). A binding-declaration edit or a source-head
    /// move partitions the keyspace, so prior findings are segregated as
    /// superseded, never presented as current. Verify never mutates the mem —
    /// any repair routes through the (later) sync brief. It then renders the
    /// deterministic, token-budgeted **tier-1 fidelity report** (group B) over
    /// the findings just recorded: grain-classed coverage with tree-anchor
    /// fan-out on its own axis, anchor-resolution %, freshness vs. both
    /// `sync_state` tokens (`signal: none` → freshness unknowable), the
    /// capability-matrix block, and the tier-3 backlog depth — aggregates always
    /// ship; heavy per-artifact lists greedy-fill under `--budget` and drop to
    /// hints (forced back in with `--include`).
    Verify(VerifyArgs),
}

/// The medium type flag for `projection init` — the CLI-facing mirror of
/// [`MediumType`] (which carries serde, not clap, derives). Decides the
/// capability matrix (D6) that filters the default binding's operations.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum MediumTypeArg {
    /// A source tree of code.
    Codebase,
    /// A directory of files (non-code).
    Filesystem,
    /// A git history.
    Git,
    /// Another mem's graph.
    Graph,
    /// Web sources (build-only this cycle — no change signal).
    Web,
}

impl MediumTypeArg {
    fn to_medium_type(self) -> MediumType {
        match self {
            MediumTypeArg::Codebase => MediumType::Codebase,
            MediumTypeArg::Filesystem => MediumType::Filesystem,
            MediumTypeArg::Git => MediumType::Git,
            MediumTypeArg::Graph => MediumType::Graph,
            MediumTypeArg::Web => MediumType::Web,
        }
    }
}

#[derive(ClapArgs, Debug)]
pub struct BriefArgs {
    /// The canonical binding id `<mem>/<stem>` (D3) — e.g. `engine/graph`.
    /// Omit (or pass `--all`) to select the next due binding by round-robin +
    /// backoff. Required with `--verify` / `--sync` (those operate on one
    /// binding's live findings/cursor, never a rotation).
    pub binding: Option<String>,
    /// Select the next due (binding, operation) pair across all bindings
    /// (round-robin + backoff) and render its brief, instead of naming one.
    /// Which operations rotate is decided by `--operation` (default: build
    /// only). Ignored with `--verify` / `--sync`.
    #[arg(long)]
    pub all: bool,
    /// Which operations the `--all` rotation considers. An operation
    /// participates only where the binding declares its block with
    /// `trigger: loop` — consent lives in the declaration. `build` (the
    /// default) keeps the classic build-only rotation; `any` rotates across
    /// every loop-declared build / sync / verify pair and renders the matching
    /// brief (the `--json` output names the picked operation).
    #[arg(long, value_enum, default_value_t = BriefOperationArg::Build, requires = "all", conflicts_with_all = ["verify", "sync"])]
    pub operation: BriefOperationArg,
    /// Render the **verify brief** (group C) for the named binding instead of
    /// the build brief: measurement + capped-adjudication instructions only.
    /// It carries no destination-mutation instruction — repairs route through
    /// the sync brief. Read-only on the mem. Mutually exclusive with `--sync`.
    #[arg(long, conflicts_with = "sync")]
    pub verify: bool,
    /// Render the **sync brief** (group C) for the named binding instead of the
    /// build brief: the sole maintenance-writer prompt, carrying both the cursor
    /// slice and the open verify findings in one brief, with the absorbed
    /// reconcile conservatism. Read-only on the mem (the agent's writes route
    /// through MCP). Mutually exclusive with `--verify`.
    #[arg(long, conflicts_with = "verify")]
    pub sync: bool,
}

/// The `--operation` value for `projection brief --all` — which operations the
/// rotation considers. CLI-facing mirror of the engine's [`OperationFilter`]
/// (which carries no clap derives).
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum BriefOperationArg {
    /// Rotate over build pairs only (the default — the classic rotation).
    Build,
    /// Rotate over sync pairs only.
    Sync,
    /// Rotate over verify pairs only.
    Verify,
    /// Rotate over every loop-declared build / sync / verify pair.
    Any,
}

impl BriefOperationArg {
    fn to_filter(self) -> OperationFilter {
        match self {
            BriefOperationArg::Build => OperationFilter::Only(OperationKind::Build),
            BriefOperationArg::Sync => OperationFilter::Only(OperationKind::Sync),
            BriefOperationArg::Verify => OperationFilter::Only(OperationKind::Verify),
            BriefOperationArg::Any => OperationFilter::Any,
        }
    }
}

#[derive(ClapArgs, Debug)]
pub struct InitArgs {
    /// Destination mem the binding writes into — the `<mem>` half of the
    /// binding id `<mem>/<stem>` and the per-mem tier the three files live under.
    #[arg(long)]
    pub mem: String,
    /// The medium pointer — a path (codebase / filesystem / git) or a mem id /
    /// URL (graph / web). Becomes the scaffolded medium's `pointer`.
    #[arg(long)]
    pub source: String,
    /// The medium type — decides the capability matrix (D6) that filters which
    /// operations the default binding declares.
    #[arg(long = "medium-type", value_enum)]
    pub medium_type: MediumTypeArg,
    /// Intent prose for the agent (the binding's `intent`). Optional.
    #[arg(long)]
    pub intent: Option<String>,
    /// Binding stem — the `<stem>` half of the binding id and the shared file
    /// name of the scaffolded medium / facet / binding. Defaults to the final
    /// path component of `--source`.
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(ClapArgs, Debug)]
pub struct MigrateArgs {
    /// Preview the produced bindings (and any warnings) without writing them
    /// to disk or removing the merged ingest files.
    #[arg(long)]
    pub dry_run: bool,
}

/// The operation `projection enable` adds to a binding. Mirror of the binding's
/// operations block: `build` is always present (required), so enabling it
/// always refuses as already-enabled; `sync` / `verify` are the enableable
/// blocks.
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum EnableOperationArg {
    /// The build operation (always present — enabling refuses as already-enabled).
    Build,
    /// The sync (maintenance-write) operation.
    Sync,
    /// The verify (measurement) operation.
    Verify,
}

impl EnableOperationArg {
    fn name(self) -> &'static str {
        match self {
            EnableOperationArg::Build => "build",
            EnableOperationArg::Sync => "sync",
            EnableOperationArg::Verify => "verify",
        }
    }
}

#[derive(ClapArgs, Debug)]
pub struct EnableArgs {
    /// The operation to enable: `build` | `sync` | `verify`.
    #[arg(value_enum)]
    pub operation: EnableOperationArg,
    /// The binding id `<mem>/<stem>` (D3) — e.g. `engine/graph`.
    pub binding: String,
}

#[derive(ClapArgs, Debug)]
pub struct AdvanceArgs {
    /// The binding id `<mem>/<stem>` (D3) — e.g. `engine/graph`.
    pub binding: String,
    /// A JSON object mapping each judged artifact id to its disposition, e.g.
    /// `'{"src/lib.rs": "worked", "src/old.rs": "irrelevant"}'`. A value may
    /// instead be an object carrying an authored rationale —
    /// `'{"src/gen.rs": {"disposition": "excluded", "rationale": "generated, no entity"}}'`
    /// — and an `excluded` verdict with a rationale is retained in the durable
    /// exclusion ledger so the artifact stops re-surfacing as `uncovered` and
    /// keeps its reasoning. Only ids the engine presented in the brief's changed
    /// slice are accepted — an unknown id refuses the whole call. Pass `'{}'` to
    /// re-present the remainder without recording anything.
    #[arg(long)]
    pub dispositions: String,
}

#[derive(ClapArgs, Debug)]
pub struct ExcludeArgs {
    /// The binding id `<mem>/<stem>` (D3) — e.g. `project/graph`.
    pub binding: String,
    /// A JSON object mapping each in-scope source artifact id to the authored
    /// rationale for excluding it, e.g.
    /// `'{"docs/legacy.md": "superseded; no entity", "vendor/x.rs": "generated"}'`.
    /// Every id must be a member of the binding's enumerable source `S(D)` — an
    /// id outside scope refuses the whole call.
    #[arg(long)]
    pub exclusions: String,
}

#[derive(ClapArgs, Debug)]
pub struct VerifyArgs {
    /// The binding id `<mem>/<stem>` (D3) — e.g. `engine/graph`.
    pub binding: String,
    /// Token budget for the tier-1 fidelity report's **heavy** content
    /// (per-artifact lists). Aggregated counts always ship in addition; heavy
    /// lists greedy-fill and drop to `## Hints` when they do not fit. Defaults
    /// to the house envelope budget.
    #[arg(long)]
    pub budget: Option<usize>,
    /// Force a heavy report section in past the budget (repeatable):
    /// `uncovered_artifacts` | `tree_fanout` | `superseded_findings`.
    #[arg(long = "include")]
    pub include: Vec<String>,
    /// Full measurement: walk the entire enumerable source `S(D)` (the
    /// rotating sample scheduler is bypassed), treat the per-run adjudication
    /// cap as unlimited, and perform the prepared-hash backfill — the
    /// report's coverage and accuracy figures are computed over everything,
    /// with no sampling or truncation caveat. Refuses (typed) when a facet's
    /// medium is non-enumerable rather than render a fabricated-complete
    /// report. Without this flag the capped/sampled loop economics are
    /// unchanged.
    #[arg(long)]
    pub full: bool,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    match args.command {
        ProjectionCommand::Brief(a) => brief(ctx, a),
        ProjectionCommand::Init(a) => init(ctx, a),
        ProjectionCommand::Migrate(a) => migrate(ctx, a),
        ProjectionCommand::Enable(a) => enable(ctx, a),
        ProjectionCommand::Advance(a) => advance(ctx, a),
        ProjectionCommand::Exclude(a) => exclude(ctx, a),
        ProjectionCommand::Verify(a) => verify(ctx, a),
    }
}

/// Map a [`RenderBriefError`] to a typed CLI error (D12). Not-found bindings /
/// facets / mediums exit `NotFound`; a malformed id is a `Validation` name
/// error; config-load and mode-unsupported failures are generic. Codes are
/// spelled as literals at each construction site so the generated error index
/// (xtask) picks them up.
fn map_brief_err(binding_id: &str, err: RenderBriefError) -> CliError {
    let message = err.to_string();
    let mapped = match &err {
        RenderBriefError::ConfigLoad(_) => {
            CliError::new(ExitKind::Generic, "PROJECTION_LOAD_FAILED", message)
        }
        // D6/AC4: the binding declares no build op — refuse with the
        // `projection enable build` remedy the error message already carries.
        RenderBriefError::BuildOperationAbsent { .. } => CliError::new(
            ExitKind::Validation,
            "PROJECTION_BUILD_NOT_ENABLED",
            message,
        ),
        // A malformed findings store while rendering a verify / sync brief.
        RenderBriefError::FindingsRead { .. } => CliError::new(
            ExitKind::Generic,
            "PROJECTION_FINDINGS_READ_FAILED",
            message,
        ),
        RenderBriefError::Resolve(inner) => match inner {
            ResolveError::BindingNotFound { .. } => {
                CliError::new(ExitKind::NotFound, "PROJECTION_NOT_FOUND", message)
            }
            ResolveError::MalformedProjectionRef { .. } => {
                CliError::new(ExitKind::Validation, "PROJECTION_INVALID_NAME", message)
            }
        },
    };
    mapped.with_details(json!({ "binding": binding_id }))
}

fn brief(ctx: &CliContext, args: BriefArgs) -> anyhow::Result<()> {
    let (_shape, root) = ctx.workspace_shape().ok_or_else(|| {
        workspace_not_initialised_error(
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)",
        )
    })?;

    let cli_engine = ctx.cli_engine_at(&root)?;
    let engine = cli_engine.base();

    // Group-C briefs: verify / sync render for one named binding (no rotation).
    // Both are read-only on the destination mem — the sync brief's repairs reach
    // the mem only when an agent acts on it through the MCP mutation surface.
    if args.verify || args.sync {
        let binding_id = args.binding.ok_or_else(|| {
            CliError::new(
                ExitKind::Validation,
                "PROJECTION_BRIEF_BINDING_REQUIRED",
                format!(
                    "`projection brief --{}` needs a binding id `<mem>/<stem>` — it renders one \
                     binding's brief, not an `--all` rotation",
                    if args.verify { "verify" } else { "sync" }
                ),
            )
        })?;
        let (rendered, operation) = if args.verify {
            (
                render_verify_brief_for(engine, &root, &binding_id),
                OperationKind::Verify,
            )
        } else {
            (
                render_sync_brief_for(engine, &root, &binding_id),
                OperationKind::Sync,
            )
        };
        let rendered = rendered.map_err(|e| map_brief_err(&binding_id, e))?;

        if ctx.json {
            print_json(&json!({ "brief": rendered, "operation": operation.as_wire() }))?;
        } else {
            print!("{rendered}");
        }
        return Ok(());
    }

    // Resolve which (binding, operation) pair to render: a named binding
    // (canonical `<mem>/<stem>`, build), or the next due pair in a round-robin
    // `--all` rotation (which advances the cursor + backoff state). The
    // rotation's operation set is `--operation` (default: build only — the
    // classic rotation, byte-stable for existing callers).
    let selected = match args.binding {
        Some(binding) if !args.all => Some((binding, OperationKind::Build)),
        _ => {
            let configs = load_pipeline_configs(&root).map_err(|e| {
                CliError::new(
                    ExitKind::Generic,
                    "PROJECTION_LOAD_FAILED",
                    format!("could not load binding store: {e}"),
                )
                .with_details(json!({ "error": e.to_string() }))
            })?;
            // Distinguish "nothing is configured" from "everything is backing
            // off". Both otherwise collapse into the same `None` from
            // `select_next_due_operation`, but the two outcomes want different
            // caller responses: an empty store is a setup prompt, a
            // backing-off pass is a no-op retry. Emit the empty-store signal
            // explicitly so a caller (the plugin router, a status display) can
            // branch on it.
            if configs.bindings.is_empty() {
                if ctx.json {
                    print_json(&json!({ "no_bindings": true }))?;
                } else {
                    println!("> **[projection] No bindings configured in this workspace yet.**");
                }
                return Ok(());
            }
            select_next_due_operation(engine, &root, &configs, args.operation.to_filter())
        }
    };

    let Some((binding_id, operation)) = selected else {
        // Every eligible pair is backing off (or not due) this pass — a valid
        // outcome, the loop's quiet yield.
        if ctx.json {
            print_json(&json!({ "skipped": true }))?;
        } else {
            println!(
                "> **[projection] Skipped — every eligible binding is backing off this pass.**"
            );
        }
        return Ok(());
    };

    // Dispatch to the selected operation's renderer: the rotation hands back
    // build / sync / verify pairs, each with its own brief.
    let rendered = match operation {
        OperationKind::Build => render_ingest_brief(engine, &root, &binding_id),
        OperationKind::Sync => render_sync_brief_for(engine, &root, &binding_id),
        OperationKind::Verify => render_verify_brief_for(engine, &root, &binding_id),
    }
    .map_err(|e| map_brief_err(&binding_id, e))?;

    if ctx.json {
        print_json(&json!({ "brief": rendered, "operation": operation.as_wire() }))?;
    } else {
        // The brief *is* the stdout content (the skill pipes it as the agent
        // prompt) — write it verbatim, no added trailing newline.
        print!("{rendered}");
    }
    Ok(())
}

/// Is `value` a single, plain path component — safe to use verbatim as a `<mem>`
/// or `<stem>` dir/file segment and as half of the binding id? Mirrors
/// `pipeline_store`'s internal component guard so `init` refuses with a clear
/// typed code up front rather than surfacing a store IO error mid-scaffold.
fn is_single_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains(':')
        && !value.contains('\0')
}

/// Derive a binding stem from a `--source` pointer: its final path component
/// (trailing slashes trimmed). `../public` → `public`; `home` → `home`;
/// `https://example.com/manual` → `manual`.
fn derive_stem(source: &str) -> String {
    source
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(source)
        .to_string()
}

/// Map a store write failure during scaffolding to a typed CLI error.
fn init_write_error(binding_id: &str, err: StoreError) -> CliError {
    CliError::new(
        ExitKind::Generic,
        "PROJECTION_INIT_FAILED",
        format!("could not scaffold binding `{binding_id}`: {err}"),
    )
    .with_details(json!({ "binding": binding_id, "error": err.to_string() }))
}

fn init(ctx: &CliContext, args: InitArgs) -> anyhow::Result<()> {
    let (_shape, root) = ctx.workspace_shape().ok_or_else(|| {
        workspace_not_initialised_error(
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)",
        )
    })?;

    let mem = args.mem;
    let stem = args
        .name
        .clone()
        .unwrap_or_else(|| derive_stem(&args.source));

    // `mem` and `stem` become three file-path components and the binding id —
    // refuse anything that is not a single plain component before touching disk.
    for (kind, value) in [("mem", mem.as_str()), ("name", stem.as_str())] {
        if !is_single_component(value) {
            return Err(CliError::new(
                ExitKind::Validation,
                "PROJECTION_INVALID_NAME",
                format!(
                    "invalid {kind} '{}': must be a single path component (no separators, \
                     traversal segments, ':' or NUL) — pass an explicit --name",
                    value.escape_default()
                ),
            )
            .with_details(json!({ "kind": kind, "value": value }))
            .into());
        }
    }

    let binding_id = format!("{mem}/{stem}");
    let medium_type = args.medium_type.to_medium_type();

    // Refuse — without touching disk — when a binding of this id already exists
    // (D8: `init` never overwrites). The binding occupies the per-mem
    // projections tier; its presence is the id-collision signal.
    let binding_path = root
        .join(".memstead")
        .join("projections")
        .join(&mem)
        .join(format!("{stem}.json"));
    if binding_path.exists() {
        return Err(CliError::new(
            ExitKind::Validation,
            "PROJECTION_EXISTS",
            format!(
                "a binding `{binding_id}` already exists at \
                 .memstead/projections/{mem}/{stem}.json — `projection init` never overwrites; \
                 choose a different --name or edit the existing binding"
            ),
        )
        .with_details(json!({ "binding": binding_id }))
        .into());
    }

    // The scaffolded record: ONE v2 binding with one inline source under the
    // binding stem. The source is scoped `**/*` (a scoped default: an
    // unscoped source — no allow patterns — would refuse at run time).
    let source = Source {
        name: stem.clone(),
        medium_type,
        pointer: args.source.clone(),
        change_detection: None,
        scope: vec![PatternEntry {
            path: "**/*".to_string(),
            mode: PatternMode::Allow,
        }],
        engagement: None,
        preparation: None,
    };

    // Matrix-filtered defaults: declare build+sync+verify, then let the
    // capability matrix strip any operation the medium cannot support. A `web`
    // source has no change signal this cycle, so sync/verify are stripped and
    // the deferral is named in `warnings[]` (operator decision 7). Every other
    // medium keeps build+sync+verify.
    let mut binding = Binding {
        version: BINDING_VERSION,
        intent: args.intent.clone(),
        sources: vec![source],
        reference_mems: Vec::new(),
        destination_mem: mem.clone(),
        deny_paths: Vec::new(),
        coverage_semantics: CoverageSemantics::Exhaustive,
        rules: None,
        prune: None,
        operations: Operations {
            build: Some(BuildOperation {
                mode: BuildMode::Discovery,
                trigger: IngestTrigger::Loop,
                batch_size: 20,
                post_actions: None,
            }),
            sync: Some(SyncOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 20,
            }),
            verify: Some(VerifyOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 20,
                adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
            }),
        },
    };

    let mut warnings: Vec<String> = Vec::new();
    if let Err(refusals) = validate_binding(&binding) {
        for r in &refusals {
            if let CapabilityError::OperationOutOfScope { operation, .. } = r {
                match *operation {
                    "sync" => binding.operations.sync = None,
                    "verify" => binding.operations.verify = None,
                    _ => {}
                }
            }
            warnings.push(r.to_string());
        }
    }

    // Prune (F1) rides the sync path — scaffold it wherever sync survived the
    // matrix filter, with the strongest guarantee the medium supports (a
    // base-retrievable / git-backed medium gets never-clobber; every
    // sync-capable medium is also base-retrievable, so this never refuses). A
    // `web` binding (sync stripped) gets no prune block.
    if binding.operations.sync.is_some() {
        binding.prune = Some(PruneConfig {
            guarantee: prune_guarantee_for_medium(medium_type),
        });
    }

    let mut operations: Vec<&str> = vec!["build"];
    if binding.operations.sync.is_some() {
        operations.push("sync");
    }
    if binding.operations.verify.is_some() {
        operations.push("verify");
    }

    // Write the one record. The id-collision refusal above already
    // guaranteed a fresh binding, so this path only runs on a clean
    // scaffold; a store IO failure surfaces the typed
    // `PROJECTION_INIT_FAILED`.
    write_binding(&root, &mem, &stem, &binding).map_err(|e| init_write_error(&binding_id, e))?;

    let created = vec![format!(".memstead/projections/{mem}/{stem}.json")];

    if ctx.json {
        // D8's pinned skill contract: { binding, created, operations, warnings }.
        print_json(&json!({
            "binding": binding_id,
            "created": created,
            "operations": operations,
            "warnings": warnings,
        }))?;
    } else {
        let mut out = format!("# Projection init\n\nScaffolded binding `{binding_id}`:\n");
        for c in &created {
            out.push_str(&format!("- `{c}`\n"));
        }
        out.push_str(&format!("\nOperations: {}\n", operations.join(", ")));
        if !warnings.is_empty() {
            out.push_str("\n## Warnings\n\n");
            for w in &warnings {
                out.push_str(&format!("- {w}\n"));
            }
        }
        print_markdown(&out);
    }
    Ok(())
}

fn map_migrate_err(err: BindingMigrateError) -> CliError {
    // Spell each `PROJECTION_*` token as a literal at its own construction site
    // so the generated error index (xtask) picks them up — a variable `code`
    // is invisible to the string-literal scanner.
    let message = err.to_string();
    match &err {
        BindingMigrateError::RefinementModeDeleted { .. } => CliError::new(
            ExitKind::Validation,
            "PROJECTION_MIGRATE_REFINEMENT",
            message,
        ),
        BindingMigrateError::MalformedProjectionRef { .. } => CliError::new(
            ExitKind::Validation,
            "PROJECTION_MIGRATE_MALFORMED_REF",
            message,
        ),
        BindingMigrateError::DanglingProjectionRef { .. }
        | BindingMigrateError::DanglingFacetRef { .. }
        | BindingMigrateError::DanglingMediumRef { .. } => CliError::new(
            ExitKind::Validation,
            "PROJECTION_MIGRATE_DANGLING_REF",
            message,
        ),
        BindingMigrateError::OrphanRecords { .. } => CliError::new(
            ExitKind::Validation,
            "PROJECTION_MIGRATE_ORPHAN_RECORDS",
            message,
        ),
    }
}

/// Does the workspace root carry a gen-1 legacy pipeline layout — the
/// pre-four-primitive `scopes|projections|ingests/` JSON folders at the root
/// (not under `.memstead/`)? Presence of any of the three marks it. This is the
/// trigger for folding the retired `pipeline migrate` conversion into
/// `projection migrate` (D10, gen-1 path).
fn has_legacy_root_layout(root: &std::path::Path) -> bool {
    ["scopes", "projections", "ingests"]
        .iter()
        .any(|d| root.join(d).is_dir())
}

/// Map a store load failure during migrate to the typed generic code.
fn migrate_load_err(err: StoreError) -> CliError {
    CliError::new(
        ExitKind::Generic,
        "PROJECTION_MIGRATE_FAILED",
        format!("could not load pipeline config: {err}"),
    )
    .with_details(json!({ "error": err.to_string() }))
}

/// Does the binding's `medium_pointer` (resolved against the workspace root)
/// point at the same location as a `reconcile-cursors.json` absolute key? Uses
/// canonicalization where both paths exist, else a lexical comparison (D10 —
/// "the binding whose medium pointer resolves to that path").
fn pointer_resolves_to(root: &std::path::Path, medium_pointer: &str, abs_path: &str) -> bool {
    let resolved = if medium_pointer.is_empty() {
        root.to_path_buf()
    } else {
        root.join(medium_pointer)
    };
    match (
        std::fs::canonicalize(&resolved),
        std::fs::canonicalize(abs_path),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => resolved == std::path::Path::new(abs_path),
    }
}

/// Scan `workspace.toml` for retired pipeline/cursor vocabulary. `projection
/// migrate` **never** writes `workspace.toml` (D10) — if it finds a stale
/// reference it returns a proposal block for the operator (or the migrating
/// session) to apply and commit explicitly, rather than rewriting it.
fn propose_workspace_toml(root: &std::path::Path) -> Option<String> {
    let path = root.join(".memstead").join("workspace.toml");
    let content = std::fs::read_to_string(path).ok()?;
    let hits: Vec<(usize, &str)> = content
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            let low = l.to_lowercase();
            low.contains("reconcile-cursors") || low.contains("ingests/") || low.contains("ingest ")
        })
        .collect();
    if hits.is_empty() {
        return None;
    }
    let mut block = String::from(
        "## Proposal: workspace.toml (NOT applied)\n\n`projection migrate` never edits \
         `workspace.toml`. It found references to retired pipeline vocabulary — review and \
         update these lines by hand, then commit:\n\n",
    );
    for (i, line) in hits {
        block.push_str(&format!("- L{}: `{}`\n", i + 1, line.trim()));
    }
    Some(block)
}

/// Consume a skill-written `reconcile-cursors.json` (D10/AC12): each
/// machine-absolute `"<mem>:<abs-path>": <sha>` entry seeds the `#synced`
/// baseline of every binding whose medium pointer resolves to that path (via
/// the engine's `set_mem_sync_state` writer — the engine owns mem-repo state),
/// then the file is **deleted** regardless of whether anything matched
/// (cursorless / unmatched bindings stay never-synced). Returns the seeded keys.
fn consume_reconcile_cursors(
    ctx: &CliContext,
    root: &std::path::Path,
) -> anyhow::Result<Vec<String>> {
    let cursor_path = root.join(".memstead").join("reconcile-cursors.json");
    if !cursor_path.exists() {
        return Ok(Vec::new());
    }
    let cursors: std::collections::BTreeMap<String, String> = std::fs::read(&cursor_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();

    let mut seeded: Vec<String> = Vec::new();
    if !cursors.is_empty() {
        let configs = load_pipeline_configs(root).map_err(migrate_load_err)?;
        let mut cli_engine = ctx.cli_engine_at(root)?;
        let engine = cli_engine.base_mut();
        for (cursor_key, sha) in &cursors {
            // Key is `"<mem>:<abs-path>"` — split on the first ':'.
            let Some((_cursor_mem, abs_path)) = cursor_key.split_once(':') else {
                continue;
            };
            for record in &configs.bindings {
                let binding_id = format!("{}/{}", record.mem, record.name);
                let Ok(resolved) = resolve_binding_run(&binding_id, &record.config) else {
                    continue;
                };
                for source in &resolved.sources {
                    if let ResolvedSource::Primary(p) = source
                        && pointer_resolves_to(root, &p.pointer, abs_path)
                    {
                        let key = format!("{binding_id}/{}#synced", p.name);
                        if engine
                            .set_mem_sync_state(
                                &resolved.destination_mem,
                                &key,
                                sha,
                                Some("projection migrate: seeded from reconcile-cursors.json"),
                            )
                            .is_ok()
                        {
                            seeded.push(key);
                        }
                    }
                }
            }
        }
    }
    // Consumed — delete regardless of matches (D10: the file is retired here).
    let _ = std::fs::remove_file(&cursor_path);
    Ok(seeded)
}

fn migrate(ctx: &CliContext, args: MigrateArgs) -> anyhow::Result<()> {
    let (_shape, root) = ctx.workspace_shape().ok_or_else(|| {
        workspace_not_initialised_error(
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)",
        )
    })?;

    // Gen-1 root-folder layout (`scopes|projections|ingests/` at the workspace
    // root) — the pre-four-primitive generation the retired `pipeline migrate`
    // command handled. Fold it in: materialize it into the four-primitive
    // `.memstead/` store first (mediums + facets + projections + ingests),
    // then fold to v2 below in the same pass. `--dry-run` reads the
    // root-folder configs directly without writing anything.
    let gen1 = has_legacy_root_layout(&root);
    if gen1 && !args.dry_run {
        migrate_legacy_pipeline(&root).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                "PROJECTION_MIGRATE_FAILED",
                format!("could not convert root-folder (gen-1) pipeline layout: {e}"),
            )
            .with_details(json!({ "error": e.to_string() }))
        })?;
    }

    let configs = if gen1 && args.dry_run {
        read_legacy_pipeline_configs(&root).map_err(migrate_load_err)?
    } else {
        load_legacy_pipeline_configs(&root).map_err(migrate_load_err)?
    };

    // Pure transforms first: any refusal (refinement / dangling / malformed /
    // orphan) aborts before a single file is touched — the migration is
    // all-or-nothing.
    //
    // Leg A (gen-2): merge each flat ingest into its projection and fold the
    // referenced facets + mediums inline — one v2 record per pipeline.
    let mut migrated = migrate_gen2_bindings(&configs).map_err(map_migrate_err)?;

    // Leg B (v1 → v2): fold every on-disk `version: 1` binding of the
    // retired three-file store the same way, in place. Source names are the
    // facet names byte-verbatim, so sync watermarks keep resolving. A
    // version-less projection file no ingest schedules is inert leftovers —
    // refused with a remedy rather than silently dropped or left to break
    // the loader. (Skipped in the gen-1 dry-run, which previews in-memory.)
    let mut already_v2 = 0usize;
    if !(gen1 && args.dry_run) {
        let generations = load_projection_generations(&root).map_err(migrate_load_err)?;
        for (mem, name, generation) in generations {
            let binding_id = format!("{mem}/{name}");
            match generation {
                ProjectionGeneration::V2 => already_v2 += 1,
                ProjectionGeneration::V1(v1) => {
                    let consumed = v1.source_facets.clone();
                    let binding = fold_v1_binding(&binding_id, &mem, v1.as_ref(), &configs)
                        .map_err(map_migrate_err)?;
                    migrated.push(memstead_base::binding_migrate::MigratedBinding {
                        id: binding_id,
                        mem,
                        name,
                        ingest_name: String::new(),
                        consumed_facets: consumed,
                        binding,
                        notes: Vec::new(),
                    });
                }
                ProjectionGeneration::VersionLess => {
                    if !migrated.iter().any(|m| m.mem == mem && m.name == name) {
                        return Err(CliError::new(
                            ExitKind::Validation,
                            "PROJECTION_MIGRATE_INERT_PROJECTION",
                            format!(
                                "projection `{binding_id}` is a version-less gen-2 file no \
                                 ingest schedules — inert leftovers the loader refuses; delete \
                                 .memstead/projections/{mem}/{name}.json (or add an ingest) and \
                                 re-run `projection migrate`"
                            ),
                        )
                        .with_details(json!({ "binding": binding_id }))
                        .into());
                    }
                }
            }
        }
        migrated.sort_by(|a, b| a.id.cmp(&b.id));

        // Every medium/facet record must have folded into some binding —
        // an orphan would be silently dropped by the tree removal, so the
        // whole migration refuses instead, naming each leftover.
        let consumed: Vec<(String, String)> = migrated
            .iter()
            .flat_map(|m| {
                m.consumed_facets
                    .iter()
                    .map(|f| (m.mem.clone(), f.clone()))
            })
            .collect();
        check_all_consumed(&configs, &consumed).map_err(map_migrate_err)?;
    }

    // Validate each produced binding against the capability matrix. A
    // capability refusal reflects a pre-existing config problem the binding
    // faithfully carries; surface it as a per-binding warning rather than
    // aborting the promotion. The folded v2 record validates directly — no
    // external resolution.
    let mut warnings: Vec<serde_json::Value> = Vec::new();
    for m in &migrated {
        if let Err(refusals) = validate_binding(&m.binding) {
            for r in refusals {
                warnings.push(json!({
                    "binding": m.id,
                    "kind": "capability",
                    "message": r.to_string(),
                }));
            }
        }
        for note in &m.notes {
            warnings.push(json!({
                "binding": m.id,
                "kind": "note",
                "message": note,
            }));
        }
    }

    // Emit to disk unless previewing: promote each projection file to its v2
    // binding in place, remove each consumed flat ingest, then remove the
    // emptied `mediums/` and `facets/` trees (every record folded — the
    // orphan check above guaranteed it).
    if !args.dry_run {
        for m in &migrated {
            write_binding(&root, &m.mem, &m.name, &m.binding).map_err(|e| {
                CliError::new(
                    ExitKind::Generic,
                    "PROJECTION_MIGRATE_FAILED",
                    format!("could not write binding `{}`: {e}", m.id),
                )
                .with_details(json!({ "binding": m.id, "error": e.to_string() }))
            })?;
            if !m.ingest_name.is_empty() {
                delete_ingest(&root, &m.ingest_name).map_err(|e| {
                    CliError::new(
                        ExitKind::Generic,
                        "PROJECTION_MIGRATE_FAILED",
                        format!("could not remove merged ingest `{}`: {e}", m.ingest_name),
                    )
                    .with_details(json!({ "ingest": m.ingest_name, "error": e.to_string() }))
                })?;
            }
        }
        remove_mediums_and_facets_trees(&root).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                "PROJECTION_MIGRATE_FAILED",
                format!("could not remove the emptied mediums/facets trees: {e}"),
            )
            .with_details(json!({ "error": e.to_string() }))
        })?;
    }

    // AC12/D10: consume `reconcile-cursors.json` (seed `#synced` baselines, then
    // delete it) and surface a `workspace.toml` proposal for any retired-vocab
    // references — never rewriting workspace.toml. Both are no-ops in `--dry-run`.
    let (seeded, proposal) = if args.dry_run {
        (Vec::new(), None)
    } else {
        (
            consume_reconcile_cursors(ctx, &root)?,
            propose_workspace_toml(&root),
        )
    };

    let bindings: Vec<&str> = migrated.iter().map(|m| m.id.as_str()).collect();
    if ctx.json {
        print_json(&json!({
            "ok": true,
            "dry_run": args.dry_run,
            "migrated": migrated.len(),
            "already_v2": already_v2,
            "bindings": bindings,
            "warnings": warnings,
            "cursors_seeded": seeded,
            "workspace_toml_proposal": proposal,
        }))?;
    } else {
        let verb = if args.dry_run {
            "Would migrate"
        } else {
            "Migrated"
        };
        let mut out = format!(
            "# Projection migration\n\n{verb} {} binding(s) to v2 ({already_v2} already v2):\n",
            migrated.len()
        );
        for id in &bindings {
            out.push_str(&format!("- `{id}`\n"));
        }
        if !warnings.is_empty() {
            out.push_str("\n## Warnings\n\n");
            for w in &warnings {
                out.push_str(&format!(
                    "- [{}] `{}`: {}\n",
                    w["kind"].as_str().unwrap_or(""),
                    w["binding"].as_str().unwrap_or(""),
                    w["message"].as_str().unwrap_or(""),
                ));
            }
        }
        if !seeded.is_empty() {
            out.push_str("\n## Baselines seeded from reconcile-cursors.json\n\n");
            for key in &seeded {
                out.push_str(&format!("- `{key}`\n"));
            }
        }
        if let Some(block) = &proposal {
            out.push('\n');
            out.push_str(block);
        }
        if !args.dry_run {
            out.push_str(
                "\nEach projection file was converted to a v2 single-record binding in place \
                 (medium + facet content folded inline, source names preserved verbatim); \
                 merged ingests and the emptied mediums/ and facets/ trees were removed.\n",
            );
        }
        print_markdown(&out);
    }
    Ok(())
}

/// A malformed binding id (not `<mem>/<stem>`, or a half that is not a single
/// plain path component) — the same shape guard `init` applies to its
/// scaffolded id, spelled here so the failure is typed before any disk touch.
fn invalid_binding_id(binding_id: &str) -> CliError {
    CliError::new(
        ExitKind::Validation,
        "PROJECTION_INVALID_NAME",
        format!(
            "invalid binding id '{}': expected `<mem>/<stem>` with each half a single path \
             component (no extra separators, traversal segments, ':' or NUL)",
            binding_id.escape_default()
        ),
    )
    .with_details(json!({ "binding": binding_id }))
}

/// Map a store IO/parse failure while enabling to a typed CLI error. The
/// missing-binding case is handled separately (existence pre-check →
/// `PROJECTION_NOT_FOUND`); this covers a present-but-unreadable/unparseable
/// binding file and write failures.
fn enable_failed(binding_id: &str, err: StoreError) -> CliError {
    CliError::new(
        ExitKind::Generic,
        "PROJECTION_ENABLE_FAILED",
        format!("could not enable operation on binding `{binding_id}`: {err}"),
    )
    .with_details(json!({ "binding": binding_id, "error": err.to_string() }))
}

fn enable(ctx: &CliContext, args: EnableArgs) -> anyhow::Result<()> {
    let (_shape, root) = ctx.workspace_shape().ok_or_else(|| {
        workspace_not_initialised_error(
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)",
        )
    })?;

    let binding_id = args.binding;
    let op = args.operation;

    // Parse the binding id `<mem>/<stem>`; refuse a malformed shape (or a half
    // that is not a single plain path component) before touching disk. Own the
    // halves so `binding_id` is free to move into JSON payloads later.
    let (mem, stem) = binding_id
        .split_once('/')
        .filter(|(m, n)| !m.is_empty() && !n.is_empty())
        .filter(|(m, n)| is_single_component(m) && is_single_component(n))
        .ok_or_else(|| invalid_binding_id(&binding_id))?;
    let mem = mem.to_string();
    let stem = stem.to_string();

    // Missing binding file → PROJECTION_NOT_FOUND (NotFound exit). A present-
    // but-unparseable file is kept apart (→ PROJECTION_ENABLE_FAILED) by this
    // existence pre-check.
    let binding_path = root
        .join(".memstead")
        .join("projections")
        .join(&mem)
        .join(format!("{stem}.json"));
    if !binding_path.exists() {
        return Err(CliError::new(
            ExitKind::NotFound,
            "PROJECTION_NOT_FOUND",
            format!(
                "no binding `{binding_id}` at .memstead/projections/{mem}/{stem}.json — \
                 scaffold one with `projection init` or migrate a legacy workspace with \
                 `projection migrate`"
            ),
        )
        .with_details(json!({ "binding": binding_id }))
        .into());
    }
    let mut binding =
        read_binding(&root, &mem, &stem).map_err(|e| enable_failed(&binding_id, e))?;

    // Already present? Refuse without a partial write. Every operation block is
    // optional now (D1/AC4), so `build` is enableable too (the remedy a
    // build-less binding's brief refusal cites).
    let already = match op {
        EnableOperationArg::Build => binding.operations.build.is_some(),
        EnableOperationArg::Sync => binding.operations.sync.is_some(),
        EnableOperationArg::Verify => binding.operations.verify.is_some(),
    };
    if already {
        return Err(CliError::new(
            ExitKind::Validation,
            "PROJECTION_OP_ALREADY_ENABLED",
            format!(
                "operation `{}` is already enabled on binding `{binding_id}` — nothing to do",
                op.name()
            ),
        )
        .with_details(json!({ "binding": binding_id, "operation": op.name() }))
        .into());
    }

    // Add the operation block with sensible defaults: `batch_size` mirrors the
    // build op's when present, else 20. Sync/verify default `trigger: manual`;
    // build defaults to a discovery/loop schedule (the common obligation shape).
    let batch_size = binding
        .operations
        .build
        .as_ref()
        .map_or(20, |b| b.batch_size);
    match op {
        EnableOperationArg::Build => {
            binding.operations.build = Some(BuildOperation {
                mode: BuildMode::Discovery,
                trigger: IngestTrigger::Loop,
                batch_size,
                post_actions: None,
            });
        }
        EnableOperationArg::Sync => {
            binding.operations.sync = Some(SyncOperation {
                trigger: IngestTrigger::Manual,
                batch_size,
            });
        }
        EnableOperationArg::Verify => {
            binding.operations.verify = Some(VerifyOperation {
                trigger: IngestTrigger::Manual,
                batch_size,
                adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
            });
        }
    }

    // Matrix validation: the v2 record carries its sources inline, so the
    // candidate validates directly — refuse if a source's medium half cannot
    // support the operation being enabled (e.g. `sync`/`verify` over a `web`
    // source). Refusals about *other* operations reflect pre-existing config
    // and do not block this enable (mirrors `migrate`'s treat-as-warning
    // posture). No write on refusal — the file stays byte-identical.
    if let Err(refusals) = validate_binding(&binding)
        && let Some(err) = refusals.iter().find(|r| {
            matches!(
                r,
                CapabilityError::OperationOutOfScope { operation, .. } if *operation == op.name()
            )
        })
    {
        return Err(CliError::new(
            ExitKind::Validation,
            "PROJECTION_CAPABILITY_UNSUPPORTED",
            err.to_string(),
        )
        .with_details(json!({ "binding": binding_id, "operation": op.name() }))
        .into());
    }

    write_binding(&root, &mem, &stem, &binding).map_err(|e| enable_failed(&binding_id, e))?;

    let mut operations: Vec<&str> = Vec::new();
    if binding.operations.build.is_some() {
        operations.push("build");
    }
    if binding.operations.sync.is_some() {
        operations.push("sync");
    }
    if binding.operations.verify.is_some() {
        operations.push("verify");
    }

    if ctx.json {
        print_json(&json!({
            "binding": binding_id,
            "enabled": op.name(),
            "operations": operations,
        }))?;
    } else {
        print_markdown(&format!(
            "# Projection enable\n\nEnabled `{}` on binding `{binding_id}`.\n\nOperations: {}\n",
            op.name(),
            operations.join(", ")
        ));
    }
    Ok(())
}

/// Map a `resolve_binding_run` failure to a typed CLI error. With inline
/// sources the dangling facet/medium refusals are gone; a malformed id is the
/// Validation-shaped name error, everything else generic.
fn map_resolve_err(binding_id: &str, err: ResolveError) -> CliError {
    let message = err.to_string();
    let mapped = match err {
        ResolveError::MalformedProjectionRef { .. } => {
            CliError::new(ExitKind::Validation, "PROJECTION_INVALID_NAME", message)
        }
        _ => CliError::new(ExitKind::Generic, "PROJECTION_ADVANCE_FAILED", message),
    };
    mapped.with_details(json!({ "binding": binding_id }))
}

/// Map an [`AdvanceError`] to a typed CLI error. The unknown-artifact refusal
/// is the D7 gate (Validation); a malformed id is a Validation-shaped name
/// error; store / engine failures are generic. Codes are spelled as literals at
/// each site so the generated error index picks them up.
fn map_advance_err(binding_id: &str, err: AdvanceError) -> CliError {
    let message = err.to_string();
    match &err {
        AdvanceError::MalformedId(_) => {
            CliError::new(ExitKind::Validation, "PROJECTION_INVALID_NAME", message)
                .with_details(json!({ "binding": binding_id }))
        }
        AdvanceError::UnknownArtifact {
            artifacts,
            suggestions,
            ..
        } => {
            // `corrected_artifacts` maps each medium-relative-looking id to
            // the workspace-relative id the slice actually presented — the
            // machine-readable half of the message's remedy.
            let corrected: serde_json::Map<String, serde_json::Value> = suggestions
                .iter()
                .map(|(supplied, corrected)| {
                    (
                        supplied.clone(),
                        serde_json::Value::String(corrected.clone()),
                    )
                })
                .collect();
            CliError::new(
                ExitKind::Validation,
                "PROJECTION_ADVANCE_UNKNOWN_ARTIFACT",
                message,
            )
            .with_details(json!({
                "binding": binding_id,
                "unknown_artifacts": artifacts,
                "corrected_artifacts": corrected,
            }))
        }
        AdvanceError::Store(_) | AdvanceError::Engine(_) => {
            CliError::new(ExitKind::Generic, "PROJECTION_ADVANCE_FAILED", message)
                .with_details(json!({ "binding": binding_id }))
        }
    }
}

fn advance(ctx: &CliContext, args: AdvanceArgs) -> anyhow::Result<()> {
    let (_shape, root) = ctx.workspace_shape().ok_or_else(|| {
        workspace_not_initialised_error(
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)",
        )
    })?;

    let binding_id = args.binding;

    // Parse the dispositions payload up front — a malformed `--dispositions`
    // refuses cheaply (before loading configs or an engine) with a typed code.
    let dispositions: std::collections::BTreeMap<String, DispositionInput> =
        serde_json::from_str(&args.dispositions).map_err(|e| {
            CliError::new(
                ExitKind::Validation,
                "PROJECTION_INVALID_DISPOSITIONS",
                format!(
                    "--dispositions must be a JSON object mapping artifact id → either a \
                     disposition string (e.g. \"worked\") or an object \
                     {{\"disposition\": \"excluded\", \"rationale\": \"...\"}}: {e}"
                ),
            )
            .with_details(json!({ "error": e.to_string() }))
        })?;

    // Find the binding by canonical id in the v1 store.
    let configs = load_pipeline_configs(&root).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "PROJECTION_ADVANCE_FAILED",
            format!("could not load pipeline config: {e}"),
        )
        .with_details(json!({ "error": e.to_string() }))
    })?;
    let record = configs
        .bindings
        .iter()
        .find(|r| format!("{}/{}", r.mem, r.name) == binding_id)
        .ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "PROJECTION_NOT_FOUND",
                format!(
                    "no binding `{binding_id}` in this workspace — scaffold one with \
                     `projection init` or migrate a legacy workspace with `projection migrate`"
                ),
            )
            .with_details(json!({ "binding": binding_id }))
        })?;

    // D6/AC4: advance is the sync (maintenance-write) path — refuse when the
    // binding declares no `sync` operation, carrying the one-command remedy
    // `projection enable sync <binding>` (which, run verbatim, makes it succeed).
    if record.config.operations.sync.is_none() {
        return Err(CliError::new(
            ExitKind::Validation,
            "PROJECTION_SYNC_NOT_ENABLED",
            format!(
                "binding `{binding_id}` has no sync operation — enable it with \
                 `memstead projection enable sync {binding_id}`"
            ),
        )
        .with_details(json!({ "binding": binding_id }))
        .into());
    }

    let resolved = resolve_binding_run(&binding_id, &record.config)
        .map_err(|e| map_resolve_err(&binding_id, e))?;

    // The engine is mutable — a completing advance writes the `#synced`
    // baseline token through the sync-state writer.
    let mut cli_engine = ctx.cli_engine_at(&root)?;
    let engine = cli_engine.base_mut();

    let outcome = advance_baseline(engine, &root, &resolved, &dispositions)
        .map_err(|e| map_advance_err(&binding_id, e))?;

    if ctx.json {
        print_json(&json!({
            "binding": outcome.binding,
            "completed": outcome.completed,
            "disposed": outcome.disposed,
            "pending": outcome.pending,
            "remainder": outcome.remainder,
            "tokens_written": outcome.tokens_written,
            "warnings": outcome.warnings,
        }))?;
    } else {
        let mut out = format!(
            "# Projection advance\n\nBinding `{}`: {} artifact(s) disposed, {} remaining.\n",
            outcome.binding, outcome.disposed, outcome.pending
        );
        if outcome.completed {
            out.push_str("\nEvery presented artifact is disposed — the sync baseline advanced.\n");
            if !outcome.tokens_written.is_empty() {
                out.push_str("\nBaseline tokens written:\n");
                for key in &outcome.tokens_written {
                    out.push_str(&format!("- `{key}`\n"));
                }
            }
        } else {
            out.push_str(
                "\nRemainder still pending — re-run `projection advance` after judging the rest \
                 (a brief re-render shows what is left).\n",
            );
        }
        if !outcome.warnings.is_empty() {
            out.push_str("\n## Warnings\n\n");
            for w in &outcome.warnings {
                out.push_str(&format!("- {w}\n"));
            }
        }
        print_markdown(&out);
    }
    Ok(())
}

/// Map an [`ExcludeError`] to a typed CLI error. The non-member refusal is the
/// S(D)-membership gate (Validation); a malformed id is a Validation-shaped name
/// error; store failures are generic. Codes are spelled as literals at each site
/// so the generated error index picks them up.
fn map_exclude_err(binding_id: &str, err: ExcludeError) -> CliError {
    let message = err.to_string();
    match &err {
        ExcludeError::MalformedId(_) => {
            CliError::new(ExitKind::Validation, "PROJECTION_INVALID_NAME", message)
                .with_details(json!({ "binding": binding_id }))
        }
        ExcludeError::NotSourceMember { artifacts, .. } => CliError::new(
            ExitKind::Validation,
            "PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER",
            message,
        )
        .with_details(json!({ "binding": binding_id, "not_source_members": artifacts })),
        ExcludeError::Store(_) => {
            CliError::new(ExitKind::Generic, "PROJECTION_EXCLUDE_FAILED", message)
                .with_details(json!({ "binding": binding_id }))
        }
    }
}

fn exclude(ctx: &CliContext, args: ExcludeArgs) -> anyhow::Result<()> {
    let (_shape, root) = ctx.workspace_shape().ok_or_else(|| {
        workspace_not_initialised_error(
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)",
        )
    })?;

    let binding_id = args.binding;

    // Parse the exclusions payload up front — a malformed `--exclusions` refuses
    // cheaply (before loading configs) with a typed code.
    let exclusions: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&args.exclusions).map_err(|e| {
            CliError::new(
                ExitKind::Validation,
                "PROJECTION_INVALID_EXCLUSIONS",
                format!(
                    "--exclusions must be a JSON object mapping in-scope artifact id → \
                     rationale string: {e}"
                ),
            )
            .with_details(json!({ "error": e.to_string() }))
        })?;

    // Find the binding by canonical id in the v1 store.
    let configs = load_pipeline_configs(&root).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "PROJECTION_EXCLUDE_FAILED",
            format!("could not load pipeline config: {e}"),
        )
        .with_details(json!({ "error": e.to_string() }))
    })?;
    let record = configs
        .bindings
        .iter()
        .find(|r| format!("{}/{}", r.mem, r.name) == binding_id)
        .ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "PROJECTION_NOT_FOUND",
                format!(
                    "no binding `{binding_id}` in this workspace — scaffold one with \
                     `projection init` or migrate a legacy workspace with `projection migrate`"
                ),
            )
            .with_details(json!({ "binding": binding_id }))
        })?;

    let resolved = resolve_binding_run(&binding_id, &record.config)
        .map_err(|e| map_resolve_err(&binding_id, e))?;

    let outcome = record_exclusions(&root, &resolved, &exclusions)
        .map_err(|e| map_exclude_err(&binding_id, e))?;

    if ctx.json {
        print_json(&json!({
            "binding": outcome.binding,
            "excluded": outcome.excluded,
            "added": outcome.added,
        }))?;
    } else {
        print_markdown(&format!(
            "# Projection exclude\n\nBinding `{}`: {} artifact(s) newly excluded, \
             {} in the ledger.\n",
            outcome.binding, outcome.added, outcome.excluded
        ));
    }
    Ok(())
}

/// Render a one-block human note for the full-enumeration scheduling decision
/// (D3), prepended to the verify report so the typed signal is never silent: a
/// scheduled full walk that fired, a not-yet-due countdown, disabled scheduling,
/// and — critically — any non-enumerable refusal. Empty for the quiet cases
/// keeps a rotating-sample run byte-clean.
fn render_full_resync_note(decision: &FullResyncDecision) -> String {
    match decision {
        FullResyncDecision::Disabled => String::new(),
        FullResyncDecision::NotDue { .. } => String::new(),
        // An explicit full measurement (`--full`): every facet walked in
        // full, scheduler bypassed, cap unlimited — stated up front so the
        // report below reads as computed, not sampled.
        FullResyncDecision::Forced { walked_facets } => {
            let facets = if walked_facets.is_empty() {
                "(no primary facets)".to_string()
            } else {
                walked_facets.join(", ")
            };
            format!(
                "> **Full measurement (`--full`)** — full-enumeration walk over: {facets}. \
                 Sampling scheduler bypassed; adjudication cap unlimited. Coverage and \
                 accuracy figures below are computed over the whole source, not sampled.\n\n"
            )
        }
        FullResyncDecision::Due {
            walked_facets,
            refused,
            ..
        } => {
            let mut s = String::from("> **Scheduled full resync (D3)** — ");
            if walked_facets.is_empty() {
                s.push_str("no enumerable facet to walk this run.");
            } else {
                s.push_str(&format!(
                    "full-enumeration coverage walk fired for: {}.",
                    walked_facets.join(", ")
                ));
            }
            for r in refused {
                s.push_str(&format!(
                    "\n> **Refused (non-enumerable):** `{}` ({}) — {}",
                    r.facet, r.medium_type, r.reason
                ));
            }
            s.push_str("\n\n");
            s
        }
    }
}

/// `projection verify <binding>` — measure fidelity and record durable findings
/// (group A). Read-only on the destination mem's *entities*; a completed run
/// records its `#verified` baseline through the engine's sync-state writer
/// (the one sanctioned post-run write — an aborted or failed run never
/// advances the token).
fn verify(ctx: &CliContext, args: VerifyArgs) -> anyhow::Result<()> {
    let (_shape, root) = ctx.workspace_shape().ok_or_else(|| {
        workspace_not_initialised_error(
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)",
        )
    })?;

    let binding_id = args.binding;

    let configs = load_pipeline_configs(&root).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "PROJECTION_VERIFY_FAILED",
            format!("could not load pipeline config: {e}"),
        )
        .with_details(json!({ "error": e.to_string() }))
    })?;
    let record = configs
        .bindings
        .iter()
        .find(|r| format!("{}/{}", r.mem, r.name) == binding_id)
        .ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "PROJECTION_NOT_FOUND",
                format!(
                    "no binding `{binding_id}` in this workspace — scaffold one with \
                     `projection init` or migrate a legacy workspace with `projection migrate`"
                ),
            )
            .with_details(json!({ "binding": binding_id }))
        })?;

    let resolved = resolve_binding_run(&binding_id, &record.config)
        .map_err(|e| map_resolve_err(&binding_id, e))?;

    // The measurement pass takes a shared engine borrow (A5 — structurally
    // incapable of a mem mutation); the mutable binding exists only for the
    // completed-run baseline write below.
    let mut cli_engine = ctx.cli_engine_at(&root)?;
    let engine = cli_engine.base_mut();

    let run = if args.full {
        verify_binding_full
    } else {
        verify_binding
    };
    let outcome = run(engine, &root, &record.config, &resolved).map_err(|e| match &e {
        // A vanished/unmounted source is a typed refusal, not a failed
        // measurement: nothing was observed, no findings were recorded,
        // and the `#verified` baseline is deliberately left untouched
        // (a transient unmount must never clobber real recorded state).
        FindingsError::SourceUnreachable { source_name, path } => CliError::new(
            ExitKind::Validation,
            "SOURCE_UNREACHABLE",
            format!(
                "verify refused for `{binding_id}`: source '{source_name}' resolves to \
                 `{path}`, which does not exist — restore or remount the source (or \
                 repoint its pointer); the recorded `#verified` baseline was left \
                 untouched"
            ),
        )
        .with_details(json!({
            "binding": binding_id,
            "source": source_name,
            "path": path,
        })),
        // `--full` over a non-enumerable medium: the existing typed
        // capability refusal — a full measurement promises complete
        // figures, so the run refuses instead of rendering a report
        // with fabricated completeness. Nothing was observed or
        // recorded.
        FindingsError::FullWalkNonEnumerable(refusal) => CliError::new(
            ExitKind::Validation,
            "PROJECTION_CAPABILITY_UNSUPPORTED",
            format!("verify --full refused for `{binding_id}`: {e}"),
        )
        .with_details(json!({
            "binding": binding_id,
            "facet": refusal.facet,
            "medium_type": refusal.medium_type,
            "reason": refusal.reason,
        })),
        _ => CliError::new(
            ExitKind::Generic,
            "PROJECTION_VERIFY_FAILED",
            format!("verify failed for `{binding_id}`: {e}"),
        )
        .with_details(json!({ "binding": binding_id, "error": e.to_string() })),
    })?;

    // The run completed — record its prepared-hash backfill: every hash the
    // pass observed for a hash-less hash-bearing anchor lands on that anchor
    // in the engine-owned anchors sidecar (measurement bookkeeping — no
    // entity content is touched). Before the report, so the rendered
    // anchor-resolution figures reflect the recorded hashes. Idempotent: a
    // pass over fully-backfilled anchors observes an empty worklist.
    let hashes_backfilled = record_anchor_hash_backfill(
        engine,
        &resolved.destination_mem,
        &outcome,
        Some("projection verify: prepared-hash backfill onto hash-less anchors"),
    )
    .map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "PROJECTION_VERIFY_BACKFILL_FAILED",
            format!(
                "verify completed and findings were recorded for `{binding_id}`, but \
                 recording the prepared-hash backfill onto the anchors sidecar failed: {e}"
            ),
        )
        .with_details(json!({ "binding": binding_id, "error": e.to_string() }))
    })?;

    // Assemble + render the tier-1 fidelity report (group B) over the findings
    // the pass just recorded. Read-only — no destination-mem mutation.
    let budget = args.budget.unwrap_or(DEFAULT_REPORT_BUDGET);
    let report = compute_fidelity_report(engine, &root, &record.config, &resolved, &outcome.key);
    let rendered = render_fidelity_report(&report, budget, &args.include);

    // The run completed — record its `#verified` baseline per observed facet
    // head through the engine's sync-state writer (the backlog-prescribed
    // writer; a failed run returned above and never reaches this).
    let verified_baseline = record_verified_baseline(
        engine,
        &resolved.destination_mem,
        &outcome,
        Some("projection verify: completed-run #verified baseline"),
    )
    .map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "PROJECTION_VERIFY_BASELINE_FAILED",
            format!(
                "verify completed and findings were recorded for `{binding_id}`, but writing \
                 the `#verified` baseline failed: {e}"
            ),
        )
        .with_details(json!({ "binding": binding_id, "error": e.to_string() }))
    })?;

    if ctx.json {
        print_json(&json!({
            "binding": outcome.binding,
            "key": {
                "binding_hash": outcome.key.binding_hash,
                "source_head": outcome.key.source_head,
            },
            "recorded": outcome.recorded,
            "superseded": outcome.superseded,
            "backlog": outcome.backlog,
            // The tier-3 full-enumeration scheduling decision (D3) — surfaced
            // (never a silent skip): whether a scheduled full walk fired, is not
            // yet due, is disabled, and any typed non-enumerable refusals.
            "full_resync": outcome.full_resync,
            // The completed run's `#verified` baseline keys, written through
            // the engine's sync-state writer.
            "verified_baseline": verified_baseline,
            // How many hash-less hash-bearing anchors gained a recorded
            // prepared-content hash this run (the completed-run backfill
            // write into the engine-owned anchors sidecar). 0 once every
            // anchor carries its hash — the backfill is idempotent.
            "hash_backfilled": hashes_backfilled,
            "report": report,
            "report_mode": rendered.mode,
            "report_markdown": rendered.markdown,
        }))?;
    } else {
        // The rendered report IS the stdout content (agent-consumable brief);
        // prepend the scheduled full-walk decision so D3's typed signal (a full
        // sweep, or a non-enumerable refusal) is never silent in human mode,
        // and append the recorded `#verified` baseline so the completed-run
        // write is visible.
        let baseline_note = if verified_baseline.is_empty() {
            String::new()
        } else {
            format!(
                "\n> **Verified baseline recorded** — {}\n",
                verified_baseline
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let backfill_note = if hashes_backfilled == 0 {
            String::new()
        } else {
            format!(
                "\n> **Prepared-hash backfill recorded** — {hashes_backfilled} hash-less \
                 anchor(s) now carry their observed prepared-content hash; subsequent \
                 verifies adjudicate them deterministically.\n"
            )
        };
        print_markdown(&format!(
            "{}{}{}{}",
            render_full_resync_note(&outcome.full_resync),
            rendered.markdown,
            backfill_note,
            baseline_note
        ));
    }
    Ok(())
}
