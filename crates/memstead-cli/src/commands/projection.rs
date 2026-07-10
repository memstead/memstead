//! `memstead projection` — the binding (projection-promotion) command tree.
//!
//! The projection is the unit: one versioned binding per source→mem obligation
//! (bundle plan `03-projection-promotion`). This slice ships the **`migrate`**
//! leaf only — gen-2 four-primitive configs (`Projection` + flat `Ingest`) →
//! v1 bindings (D10, gen-2 path). The `brief` / `init` / `advance` / `enable`
//! leaves land in later sessions; `memstead ingest` and `memstead pipeline`
//! stay for now and retire with them.
//!
//! Errors carry `PROJECTION_*` wire tokens (D12); the missing-workspace path is
//! single-sourced through [`crate::setup::workspace_not_initialised_error`].

use clap::{Args as ClapArgs, Subcommand};
use serde_json::json;

use memstead_base::binding::validate_binding;
use memstead_base::binding_migrate::{
    BindingMigrateError, migrate_gen2_bindings, resolve_migrated_binding,
};
use memstead_base::pipeline_store::{delete_ingest, load_pipeline_configs, write_binding};

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
    /// Migrate gen-2 four-primitive configs (per-mem `Projection` + flat
    /// `Ingest`) into v1 bindings, merging each ingest into the projection its
    /// `projection` ref names. The binding takes the projection's file
    /// identity (`.memstead/projections/<mem>/<stem>.json`); the merged ingest
    /// is removed. `refinement` mode and dangling projection refs refuse with a
    /// typed error. Use `--dry-run` to preview without writing.
    Migrate(MigrateArgs),
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
        ProjectionCommand::Migrate(a) => migrate(ctx, a),
    }
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
