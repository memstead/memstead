use std::sync::Arc;

use clap::Parser;
use serde_json::json;

use memstead_base::render;
use memstead_schema::Schema;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

/// Describe one type, or list all types when no name is given.
///
/// Resolves the schema against the workspace's writable mem when
/// exactly one is loaded (so the catalogue agents read matches the
/// schema `memstead create` will validate against). Multi-mem workspaces
/// pin the choice via `--mem <name>`. Workspaces with zero writable
/// mems fall back to the engine built-in default so the cold-start
/// probe-from-scratch flow keeps working.
#[derive(Parser, Debug)]
pub struct Args {
    pub name: Option<String>,

    /// Resolve the schema from this writable mem's pin. Required
    /// when the workspace has more than one writable mem; defaults
    /// to the lone writable mem otherwise.
    #[arg(long)]
    pub mem: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let resolved = resolve_schema(ctx, args.mem.as_deref())?;
    let schema = &resolved.schema;
    let (schema_name, schema_version) = schema.id();
    let schema_label = format!("{schema_name}@{schema_version}");
    // The condition line rides ABOVE the schema label, so a reader meets
    // "this is not your workspace's schema" before the catalogue itself.
    let notice = resolved.condition.as_ref().map(|c| c.notice());

    let md = match args.name.as_deref() {
        None | Some("") => {
            let mut out = render::render_type_catalog_markdown_for(schema);
            out.insert_str(0, &format!("**Schema:** `{schema_label}`\n\n"));
            if let Some(n) = &notice {
                out.insert_str(0, &format!("{n}\n\n"));
            }
            out
        }
        Some(name) => match schema.get_type(name) {
            Some(td) => {
                let mut out = render::render_type_info_markdown(&td);
                out.insert_str(0, &format!("**Schema:** `{schema_label}`\n\n"));
                if let Some(n) = &notice {
                    out.insert_str(0, &format!("{n}\n\n"));
                }
                out
            }
            None => {
                let mut known: Vec<&str> = schema.types.keys().map(String::as_str).collect();
                known.sort();
                // The refusal arm needs the condition MORE than the success
                // arms do, not less. Without it a user whose own schema
                // declares the type is told it does not exist, attributed to
                // a schema that is not theirs, with the quarantine unmentioned
                // on both channels — the sharpest form of the defect this
                // command was fixed for.
                let message = match &notice {
                    Some(n) => format!(
                        "Unknown type: {name} (schema {schema_label}). Known types: {}\n\n{n}",
                        known.join(", ")
                    ),
                    None => format!(
                        "Unknown type: {name} (schema {schema_label}). Known types: {}",
                        known.join(", ")
                    ),
                };
                return Err(
                    CliError::new(ExitKind::Generic, "UNKNOWN_ENTITY_TYPE", message)
                        .with_details(json!({
                            "name": name,
                            "schema_ref": schema_label,
                            "declared": known,
                            "fallback": resolved.condition.as_ref().map(|c| json!({
                                "code": c.code(),
                                "detail": c.notice(),
                            })),
                        }))
                        .into(),
                );
            }
        },
    };

    if ctx.json {
        print_json(&json!({
            "markdown": md,
            "schema": schema_label,
            // Absent on the healthy path and on the cold-start probe; present
            // whenever the schema above is a stand-in, so a machine consumer
            // branches on the code rather than parsing the prose.
            "fallback": resolved.condition.as_ref().map(|c| json!({
                "code": c.code(),
                "detail": c.notice(),
            })),
        }))?;
    } else {
        print_markdown(&md);
    }
    Ok(())
}

/// Resolve which schema `memstead type` describes.
///
/// Resolution order:
/// 1. `--mem <name>` supplied: error if the name matches no loaded
///    mem (writable OR RO); otherwise use that mem's schema.
///    Schema introspection is a read-only operation — RO mounts are
///    first-class read targets, so resolving against them is admitted.
/// 2. Exactly one writable mem loaded: use its schema (the common case
///    for the bare `memstead type` invocation, since the implicit-mem
///    default still picks a writable target — RO mounts are explicit-
///    only via `--mem`).
/// 3. Multiple writable mems loaded: error with an actionable message
///    listing them and pointing at `--mem`.
/// 4. Zero writable mems (no workspace, cold-start probe): fall back
///    to the engine built-in default so the catalogue is still readable.
///
/// Every fallback reports the condition that produced it, because the
/// three are not one situation. A fallback may stand in for an ABSENT
/// workspace; it may not stand in for a workspace whose mems the engine
/// refused to load. Those two were indistinguishable here until
/// 2026-08-27 — both yield zero writable mems — and collapsing them is
/// what turned a correct, loud quarantine into a quiet wrong answer three
/// surfaces later: the command printed the built-in default's name,
/// version and whole type catalogue over a workspace whose only mem was
/// quarantined, saying nothing about it.
fn resolve_schema(ctx: &CliContext, mem: Option<&str>) -> anyhow::Result<Resolved> {
    let engine = match ctx.cli_engine() {
        Ok(e) => e,
        // No workspace at all: the cold-start probe. Silent by design —
        // here the built-in default IS the answer, not a stand-in, and a
        // warning on the healthy path trains readers to ignore warnings.
        Err(_) => return Ok(Resolved::cold_start()),
    };
    let engine: memstead_base::Engine = engine.into_base();
    let writable: Vec<&str> = engine.writable_mem_names();
    let all_loaded: Vec<&str> = engine.mem_names();
    let resolved_mem: &str = match mem {
        Some(name) => {
            // F25: `--mem` resolves against every loaded
            // mem, not just the writable subset. Schema lookup is
            // read-only; RO mounts have schemas worth introspecting.
            if !all_loaded.contains(&name) {
                let known = if all_loaded.is_empty() {
                    "no mems loaded".to_string()
                } else {
                    format!("known mems: [{}]", all_loaded.join(", "))
                };
                return Err(CliError {
                    code: "UNKNOWN_MEM",
                    kind: ExitKind::NotFound,
                    message: format!("unknown mem: {name} — {known}"),
                    details: Some(json!({ "mem": name, "known_mems": all_loaded })),
                }
                .into());
            }
            name
        }
        None => match writable.len() {
            0 => {
                // Inside a workspace with nothing writable loaded. If the
                // engine quarantined mems, that is the reason, and the
                // engine already produced the typed reason and the repair
                // command — surface them rather than restating them.
                let quarantined: Vec<QuarantineNote> = engine
                    .quarantined_mems()
                    .iter()
                    .map(|q| QuarantineNote {
                        mem: q.mount.mem.clone(),
                        reason_code: q.reason_code.clone(),
                        reason: q.reason_message.clone(),
                    })
                    .collect();
                return Ok(Resolved {
                    schema: Schema::builtin_default(),
                    condition: if quarantined.is_empty() {
                        Some(FallbackCondition::NoWritableMem)
                    } else {
                        Some(FallbackCondition::AllQuarantined { quarantined })
                    },
                });
            }
            1 => writable[0],
            _ => {
                // When every writable mem pins the same schema, the
                // type definition is identical regardless of which mem
                // answers — drop the `--mem` ceremony and pick any.
                // Refuse only when the writable mems pin *different*
                // schemas (the answer would genuinely depend on the
                // choice; rendering one mem's type as the answer for
                // all would be silently wrong).
                let schemas = engine.schemas();
                let schema_id = |v: &str| {
                    schemas
                        .get(v)
                        .map(|s| (s.manifest.name.clone(), s.version.clone()))
                };
                let first = schema_id(writable[0]);
                let all_same = first.is_some() && writable.iter().all(|v| schema_id(v) == first);
                if all_same {
                    writable[0]
                } else {
                    return Err(CliError::new(
                        ExitKind::Validation,
                        "AMBIGUOUS_MEM",
                        format!(
                            "writable mems pin different schemas ([{}]) — pass `--mem <name>` to pick one",
                            writable.join(", ")
                        ),
                    )
                    .with_details(json!({ "mems": writable }))
                    .into());
                }
            }
        },
    };
    match engine.schemas().get(resolved_mem).cloned() {
        Some(schema) => Ok(Resolved {
            schema,
            condition: None,
        }),
        // The third fallback, silent in exactly the same way: a mem
        // resolved fine but carries no schema entry. Named here because a
        // fix aimed only at the two paths the field report hit would leave
        // this one printing a default as though it were the mem's own.
        None => Ok(Resolved {
            schema: Schema::builtin_default(),
            condition: Some(FallbackCondition::MemHasNoSchema {
                mem: resolved_mem.to_string(),
            }),
        }),
    }
}

/// A resolved schema plus the condition that produced it, when the
/// answer is a fallback rather than the workspace's own pin.
struct Resolved {
    schema: Arc<Schema>,
    /// `None` when the schema is genuinely the resolved mem's own.
    condition: Option<FallbackCondition>,
}

impl Resolved {
    /// The cold-start probe: no workspace, so the built-in default is the
    /// answer rather than a stand-in for one.
    fn cold_start() -> Self {
        Self {
            schema: Schema::builtin_default(),
            condition: None,
        }
    }
}

/// One quarantined mem, as the engine reported it.
struct QuarantineNote {
    mem: String,
    reason_code: String,
    /// The engine's own message, repair command included.
    reason: String,
}

/// Why a fallback schema is being shown instead of a workspace's own.
enum FallbackCondition {
    /// A workspace loaded, its mems are quarantined, and these are they.
    AllQuarantined { quarantined: Vec<QuarantineNote> },
    /// A workspace loaded with no writable mem and nothing quarantined.
    NoWritableMem,
    /// A mem resolved but carries no schema entry.
    MemHasNoSchema { mem: String },
}

impl FallbackCondition {
    /// The line the command prints above the catalogue, so a reader never
    /// takes a fallback for the workspace's pinned schema.
    fn notice(&self) -> String {
        match self {
            Self::AllQuarantined { quarantined } => {
                let mut out = String::from(
                    "**No mem is serving in this workspace** — the schema below is the \
                     engine built-in default, not this workspace's own. \
                     Quarantined:\n",
                );
                for q in quarantined {
                    out.push_str(&format!(
                        "\n- `{}` ({}): {}\n",
                        q.mem, q.reason_code, q.reason
                    ));
                }
                out
            }
            Self::NoWritableMem => "**No writable mem is loaded in this workspace** — the \
                 schema below is the engine built-in default, not this workspace's own."
                .to_string(),
            Self::MemHasNoSchema { mem } => format!(
                "**Mem `{mem}` carries no schema entry** — the schema below is the engine \
                 built-in default, not this mem's own."
            ),
        }
    }

    /// Machine-readable twin for the `--json` envelope.
    fn code(&self) -> &'static str {
        match self {
            Self::AllQuarantined { .. } => "ALL_MEMS_QUARANTINED",
            Self::NoWritableMem => "NO_WRITABLE_MEM",
            Self::MemHasNoSchema { .. } => "MEM_HAS_NO_SCHEMA",
        }
    }
}
