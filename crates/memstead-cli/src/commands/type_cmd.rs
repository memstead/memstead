use std::sync::Arc;

use clap::Parser;
use serde_json::json;

use memstead_base::render;
use memstead_schema::Schema;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// Describe one type, or list all types when no name is given.
///
/// Resolves the schema against the workspace's writable vault when
/// exactly one is loaded (so the catalogue agents read matches the
/// schema `memstead create` will validate against). Multi-vault workspaces
/// pin the choice via `--vault <name>`. Workspaces with zero writable
/// vaults fall back to the engine built-in default so the cold-start
/// probe-from-scratch flow keeps working.
#[derive(Parser, Debug)]
pub struct Args {
    pub name: Option<String>,

    /// Resolve the schema from this writable vault's pin. Required
    /// when the workspace has more than one writable vault; defaults
    /// to the lone writable vault otherwise.
    #[arg(long)]
    pub vault: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let schema = resolve_schema(ctx, args.vault.as_deref())?;
    let (schema_name, schema_version) = schema.id();
    let schema_label = format!("{schema_name}@{schema_version}");

    let md = match args.name.as_deref() {
        None | Some("") => {
            let mut out = render::render_type_catalog_markdown_for(&schema);
            out.insert_str(0, &format!("**Schema:** `{schema_label}`\n\n"));
            out
        }
        Some(name) => match schema.get_type(name) {
            Some(td) => {
                let mut out = render::render_type_info_markdown(&td);
                out.insert_str(0, &format!("**Schema:** `{schema_label}`\n\n"));
                out
            }
            None => {
                let mut known: Vec<&str> =
                    schema.types.keys().map(String::as_str).collect();
                known.sort();
                return Err(CliError::new(
                    ExitKind::Generic,
                    "UNKNOWN_ENTITY_TYPE",
                    format!(
                        "Unknown type: {name} (schema {schema_label}). \
                         Known types: {}",
                        known.join(", ")
                    ),
                )
                .with_details(json!({
                    "name": name,
                    "schema_ref": schema_label,
                    "declared": known,
                }))
                .into());
            }
        },
    };

    if ctx.json {
        print_json(&json!({
            "markdown": md,
            "schema": schema_label,
        }))?;
    } else {
        print_markdown(&md);
    }
    Ok(())
}

/// Resolve which schema `memstead type` describes.
///
/// Resolution order:
/// 1. `--vault <name>` supplied: error if the name matches no loaded
///    vault (writable OR RO); otherwise use that vault's schema.
///    Schema introspection is a read-only operation — RO mounts are
///    first-class read targets, so resolving against them is admitted.
/// 2. Exactly one writable vault loaded: use its schema (the common case
///    for the bare `memstead type` invocation, since the implicit-vault
///    default still picks a writable target — RO mounts are explicit-
///    only via `--vault`).
/// 3. Multiple writable vaults loaded: error with an actionable message
///    listing them and pointing at `--vault`.
/// 4. Zero writable vaults (no workspace, cold-start probe): fall back
///    to the engine built-in default so the catalogue is still readable.
fn resolve_schema(
    ctx: &CliContext,
    vault: Option<&str>,
) -> anyhow::Result<Arc<Schema>> {
    let engine = match ctx.cli_engine() {
        Ok(e) => e,
        // No workspace at all: cold-start probe — fall through to
        // built-in default.
        Err(_) => return Ok(Schema::builtin_default()),
    };
    let engine: memstead_base::Engine = match engine {
        #[cfg(feature = "vault-repo")]
        CliEngine::VaultRepo(e) => e,
        CliEngine::Filesystem(e) => e,
    };
    let writable: Vec<&str> = engine.writable_vault_names();
    let all_loaded: Vec<&str> = engine.vault_names();
    let resolved_vault: &str = match vault {
        Some(name) => {
            // F25: `--vault` resolves against every loaded
            // vault, not just the writable subset. Schema lookup is
            // read-only; RO mounts have schemas worth introspecting.
            if !all_loaded.iter().any(|w| *w == name) {
                let known = if all_loaded.is_empty() {
                    "no vaults loaded".to_string()
                } else {
                    format!("known vaults: [{}]", all_loaded.join(", "))
                };
                return Err(CliError {
                    code: "UNKNOWN_VAULT",
                    kind: ExitKind::NotFound,
                    message: format!("unknown vault: {name} — {known}"),
                    details: Some(json!({ "vault": name, "known_vaults": all_loaded })),
                }
                .into());
            }
            name
        }
        None => match writable.len() {
            0 => return Ok(Schema::builtin_default()),
            1 => writable[0],
            _ => {
                // When every writable vault pins the same schema, the
                // type definition is identical regardless of which vault
                // answers — drop the `--vault` ceremony and pick any.
                // Refuse only when the writable vaults pin *different*
                // schemas (the answer would genuinely depend on the
                // choice; rendering one vault's type as the answer for
                // all would be silently wrong).
                let schemas = engine.schemas();
                let schema_id = |v: &str| {
                    schemas
                        .get(v)
                        .map(|s| (s.manifest.name.clone(), s.version.clone()))
                };
                let first = schema_id(writable[0]);
                let all_same =
                    first.is_some() && writable.iter().all(|v| schema_id(v) == first);
                if all_same {
                    writable[0]
                } else {
                    return Err(CliError::new(
                        ExitKind::Validation,
                        "AMBIGUOUS_VAULT",
                        format!(
                            "writable vaults pin different schemas ([{}]) — pass `--vault <name>` to pick one",
                            writable.join(", ")
                        ),
                    )
                    .with_details(json!({ "vaults": writable }))
                    .into());
                }
            }
        },
    };
    Ok(engine
        .schemas()
        .get(resolved_vault)
        .cloned()
        .unwrap_or_else(|| Schema::builtin_default()))
}
