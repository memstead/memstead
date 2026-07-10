//! `memstead projection` — the binding (projection-promotion) command tree.
//!
//! The projection is the unit: one versioned binding per source→mem obligation
//! (bundle plan `03-projection-promotion`). This slice ships the **`init`** and
//! **`migrate`** leaves — `init` scaffolds a fresh v1 binding non-interactively
//! (D8), `migrate` promotes gen-2 four-primitive configs (`Projection` + flat
//! `Ingest`) into v1 bindings (D10, gen-2 path). The `brief` / `advance` /
//! `enable` leaves land in later sessions; `memstead ingest` and
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
use memstead_base::ingest::resolve::ResolvedPrimarySource;
use memstead_base::pipeline::{
    Facet, IngestTrigger, Medium, MediumType, PatternEntry, PatternMode,
};
use memstead_base::pipeline_store::{
    delete_ingest, load_pipeline_configs, write_binding, write_facet, write_medium,
};
use memstead_base::workspace_store::StoreError;

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

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    match args.command {
        ProjectionCommand::Init(a) => init(ctx, a),
        ProjectionCommand::Migrate(a) => migrate(ctx, a),
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

    let configs = load_pipeline_configs(&root).map_err(|e| {
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
