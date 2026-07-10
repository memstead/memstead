//! `memstead projection` — the binding (projection-promotion) command tree.
//!
//! The projection is the unit: one versioned binding per source→mem obligation
//! (bundle plan `03-projection-promotion`). This slice ships the **`init`**,
//! **`migrate`**, and **`enable`** leaves — `init` scaffolds a fresh v1 binding
//! non-interactively (D8), `migrate` promotes gen-2 four-primitive configs
//! (`Projection` + flat `Ingest`) into v1 bindings (D10, gen-2 path), and
//! `enable` adds a missing `build` / `sync` / `verify` operation block to an
//! existing binding (D6 — the remedy a refused mutating op cites). The `brief`
//! / `advance` leaves land in later sessions; `memstead ingest` and
//! `memstead pipeline` stay for now and retire with them.
//!
//! Errors carry `PROJECTION_*` wire tokens (D12); the missing-workspace path is
//! single-sourced through [`crate::setup::workspace_not_initialised_error`].

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use serde_json::json;

use memstead_base::binding::{
    BINDING_VERSION, BindingV1, BuildMode, BuildOperation, CapabilityError, CoverageSemantics,
    Operations, ResolvedBinding, SyncOperation, VerifyOperation, validate_binding,
};
use memstead_base::binding_migrate::{
    BindingMigrateError, migrate_gen2_bindings, resolve_migrated_binding,
};
use memstead_base::ingest::advance::{AdvanceError, advance_baseline};
use memstead_base::ingest::resolve::{
    ResolveError, ResolvedPrimarySource, resolve_binding, resolve_binding_run,
};
use memstead_base::pipeline::{
    Facet, IngestTrigger, Medium, MediumType, PatternEntry, PatternMode,
};
use memstead_base::pipeline_store::{
    delete_ingest, load_legacy_pipeline_configs, load_pipeline_configs, read_binding,
    write_binding, write_facet, write_medium,
};
use memstead_base::workspace_store::StoreError;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine, workspace_not_initialised_error};

#[derive(ClapArgs, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: ProjectionCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProjectionCommand {
    /// Scaffold a fresh v1 binding non-interactively: a `Medium`, a `Facet`,
    /// and a v1 binding under `.memstead/{mediums,facets,projections}/<mem>/`.
    /// All inputs are flags — no prompts ever (parity across callers). The
    /// default binding declares build+sync+verify capability-permitting (D6):
    /// a `web` source scaffolds build-only, with the deferral named in
    /// `warnings[]`. Refuses `PROJECTION_EXISTS` (without touching disk) when a
    /// binding of the same id already exists — never overwrites.
    Init(InitArgs),
    /// Migrate gen-2 four-primitive configs (per-mem `Projection` + flat
    /// `Ingest`) into v1 bindings, merging each ingest into the projection its
    /// `projection` ref names. The binding takes the projection's file
    /// identity (`.memstead/projections/<mem>/<stem>.json`); the merged ingest
    /// is removed. `refinement` mode and dangling projection refs refuse with a
    /// typed error. Use `--dry-run` to preview without writing.
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
    /// `'{"src/lib.rs": "worked", "src/old.rs": "irrelevant"}'`. Only ids the
    /// engine presented in the brief's changed slice are accepted — an unknown
    /// id refuses the whole call. Pass `'{}'` to re-present the remainder
    /// without recording anything.
    #[arg(long)]
    pub dispositions: String,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    match args.command {
        ProjectionCommand::Init(a) => init(ctx, a),
        ProjectionCommand::Migrate(a) => migrate(ctx, a),
        ProjectionCommand::Enable(a) => enable(ctx, a),
        ProjectionCommand::Advance(a) => advance(ctx, a),
    }
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

    // The scaffolded triple. The medium and facet share the binding stem as
    // their file identity — one tidy `mediums`/`facets`/`projections` triple per
    // obligation. The facet is scoped `**/*` (a scoped default: an unscoped
    // facet — no allow patterns — would refuse at run time).
    let medium = Medium {
        name: stem.clone(),
        medium_type,
        pointer: args.source.clone(),
        change_detection: None,
    };
    let scope = vec![PatternEntry {
        path: "**/*".to_string(),
        mode: PatternMode::Allow,
    }];
    let facet = Facet {
        name: stem.clone(),
        medium: stem.clone(),
        scope: scope.clone(),
        engagement: None,
        preparation: None,
    };

    // Matrix-filtered defaults (D6): declare build+sync+verify, then let the
    // capability matrix strip any operation the medium cannot support. A `web`
    // source has no change signal this cycle, so sync/verify are stripped and
    // the deferral is named in `warnings[]` (operator decision 7). Every other
    // medium keeps build+sync+verify.
    let mut binding = BindingV1 {
        version: BINDING_VERSION,
        intent: args.intent.clone(),
        source_facets: vec![stem.clone()],
        reference_mems: Vec::new(),
        destination_mem: mem.clone(),
        deny_paths: Vec::new(),
        coverage_semantics: CoverageSemantics::Exhaustive,
        rules: None,
        operations: Operations {
            build: BuildOperation {
                mode: BuildMode::Discovery,
                trigger: IngestTrigger::Loop,
                batch_size: 20,
                post_actions: None,
            },
            sync: Some(SyncOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 20,
            }),
            verify: Some(VerifyOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 20,
            }),
        },
    };

    let resolved = ResolvedBinding {
        binding: binding.clone(),
        primary_sources: vec![ResolvedPrimarySource {
            facet_ref: stem.clone(),
            medium: stem.clone(),
            medium_type,
            medium_pointer: args.source.clone(),
            declared_change_detection: None,
            scope,
            preparation: None,
        }],
    };

    let mut warnings: Vec<String> = Vec::new();
    if let Err(refusals) = validate_binding(&resolved) {
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

    let mut operations: Vec<&str> = vec!["build"];
    if binding.operations.sync.is_some() {
        operations.push("sync");
    }
    if binding.operations.verify.is_some() {
        operations.push("verify");
    }

    // Write the triple. The id-collision refusal above already guaranteed a
    // fresh binding, so this path only runs on a clean scaffold; a store IO
    // failure surfaces the typed `PROJECTION_INIT_FAILED`.
    write_medium(&root, &mem, &stem, &medium).map_err(|e| init_write_error(&binding_id, e))?;
    write_facet(&root, &mem, &stem, &facet).map_err(|e| init_write_error(&binding_id, e))?;
    write_binding(&root, &mem, &stem, &binding).map_err(|e| init_write_error(&binding_id, e))?;

    let created = vec![
        format!(".memstead/mediums/{mem}/{stem}.json"),
        format!(".memstead/facets/{mem}/{stem}.json"),
        format!(".memstead/projections/{mem}/{stem}.json"),
    ];

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
        BindingMigrateError::DanglingProjectionRef { .. } => CliError::new(
            ExitKind::Validation,
            "PROJECTION_MIGRATE_DANGLING_REF",
            message,
        ),
    }
}

fn migrate(ctx: &CliContext, args: MigrateArgs) -> anyhow::Result<()> {
    let (_shape, root) = ctx.workspace_shape().ok_or_else(|| {
        workspace_not_initialised_error(
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)",
        )
    })?;

    let configs = load_legacy_pipeline_configs(&root).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "PROJECTION_MIGRATE_FAILED",
            format!("could not load pipeline config: {e}"),
        )
        .with_details(json!({ "error": e.to_string() }))
    })?;

    // Pure transform first: any refusal (refinement / dangling / malformed)
    // aborts before a single file is touched — the migration is all-or-nothing.
    let migrated = migrate_gen2_bindings(&configs).map_err(map_migrate_err)?;

    // Validate each produced binding against the D6 capability matrix. A
    // capability refusal reflects a pre-existing config problem the binding
    // faithfully carries; surface it as a per-binding warning rather than
    // aborting the promotion.
    let mut warnings: Vec<serde_json::Value> = Vec::new();
    for m in &migrated {
        match resolve_migrated_binding(&configs, &m.ingest_name, m.binding.clone()) {
            Ok(resolved) => {
                if let Err(refusals) = validate_binding(&resolved) {
                    for r in refusals {
                        warnings.push(json!({
                            "binding": m.id,
                            "kind": "capability",
                            "message": r.to_string(),
                        }));
                    }
                }
            }
            Err(e) => warnings.push(json!({
                "binding": m.id,
                "kind": "resolve",
                "message": e.to_string(),
            })),
        }
        for note in &m.notes {
            warnings.push(json!({
                "binding": m.id,
                "kind": "note",
                "message": note,
            }));
        }
    }

    // Emit to disk unless previewing: promote each projection file to its v1
    // binding in place, then remove the consumed flat ingest.
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

    let bindings: Vec<&str> = migrated.iter().map(|m| m.id.as_str()).collect();
    if ctx.json {
        print_json(&json!({
            "ok": true,
            "dry_run": args.dry_run,
            "migrated": migrated.len(),
            "bindings": bindings,
            "warnings": warnings,
        }))?;
    } else {
        let verb = if args.dry_run {
            "Would migrate"
        } else {
            "Migrated"
        };
        let mut out = format!(
            "# Projection migration\n\n{verb} {} binding(s) to v1:\n",
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
        if !args.dry_run {
            out.push_str(
                "\nEach projection file was promoted to a v1 binding in place and its merged \
                 ingest removed.\n",
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

    // Already present? Refuse without a partial write. `build` is required, so
    // it is always present — enabling it always lands here.
    let already = match op {
        EnableOperationArg::Build => true,
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

    // Add the operation block with sensible defaults: `trigger: manual`,
    // `batch_size` mirroring the build op's. `build` is unreachable here (it is
    // always already-enabled above).
    let batch_size = binding.operations.build.batch_size;
    match op {
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
            });
        }
        EnableOperationArg::Build => unreachable!("build is always already-enabled"),
    }

    // Matrix validation (D6): resolve the candidate binding (facets → mediums,
    // in the binding-id's `<mem>` tier) and refuse if the medium cannot support
    // the operation being enabled — e.g. `sync`/`verify` over a `web` source.
    // Refusals about *other* operations reflect pre-existing config and do not
    // block this enable (mirrors `migrate`'s treat-as-warning posture). No write
    // on refusal — the file stays byte-identical.
    let configs = load_legacy_pipeline_configs(&root).map_err(|e| enable_failed(&binding_id, e))?;
    let resolved = resolve_binding(&configs, &binding_id, &binding).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "PROJECTION_ENABLE_FAILED",
            format!("could not resolve binding `{binding_id}` for validation: {e}"),
        )
        .with_details(json!({ "binding": binding_id, "error": e.to_string() }))
    })?;
    if let Err(refusals) = validate_binding(&resolved)
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

    let mut operations: Vec<&str> = vec!["build"];
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

/// Map a `resolve_binding_run` failure (dangling facet/medium, malformed id)
/// to a typed CLI error. `IngestNotFound` / `ProjectionNotFound` cannot arise
/// from the binding resolver (the binding *is* the declaration), so they fall
/// through to the generic advance-failure code.
fn map_resolve_err(binding_id: &str, err: ResolveError) -> CliError {
    let message = err.to_string();
    let mapped = match err {
        ResolveError::FacetNotFound { .. } => {
            CliError::new(ExitKind::NotFound, "PROJECTION_FACET_NOT_FOUND", message)
        }
        ResolveError::MediumNotFound { .. } => {
            CliError::new(ExitKind::NotFound, "PROJECTION_MEDIUM_NOT_FOUND", message)
        }
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
        AdvanceError::UnknownArtifact { artifacts, .. } => CliError::new(
            ExitKind::Validation,
            "PROJECTION_ADVANCE_UNKNOWN_ARTIFACT",
            message,
        )
        .with_details(json!({ "binding": binding_id, "unknown_artifacts": artifacts })),
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
    let dispositions: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&args.dispositions).map_err(|e| {
            CliError::new(
                ExitKind::Validation,
                "PROJECTION_INVALID_DISPOSITIONS",
                format!(
                    "--dispositions must be a JSON object mapping artifact id → disposition \
                     string: {e}"
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
    let resolved = resolve_binding_run(&configs, &binding_id, &record.config)
        .map_err(|e| map_resolve_err(&binding_id, e))?;

    // The engine is mutable — a completing advance writes the `#synced`
    // baseline token through the sync-state writer.
    let mut cli_engine = ctx.cli_engine_at(&root)?;
    let engine = match &mut cli_engine {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(e) => e,
        CliEngine::Filesystem(e) => e,
    };

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
