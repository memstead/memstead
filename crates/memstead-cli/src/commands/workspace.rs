//! `memstead workspace ...` — workspace introspection and configuration commands.
//!
//! Subcommand families:
//!
//! - `dump` — emit a JSON document describing every writable mem, the
//!   schema each is pinned to, and per-mem opaque snapshot tokens.
//!   Storage-agnostic contract consumed by the Claude-Code ingest
//!   plugin; versioned (`format = "workspace-dump/v0"`).
//! - `allow-create / revoke-create / allow-delete / revoke-delete /
//!   grant-cross-link / revoke-cross-link / set-mutations` — write
//!   surface for `.memstead/workspace.toml`'s engine-bound sections.
//!   Mirrors what an operator would hand-edit, with `toml_edit`
//!   preserving comments and formatting on sections the CLI doesn't
//!   touch. The principle from AGENTS.md is that every engine
//!   operation should be reachable via the CLI; this family closes the
//!   asymmetry for workspace configuration.
//!
//! `[plugin.*]` sections stay operator-edited — they are opaque
//! pass-through to the engine and the CLI has no business knowing
//! their key shapes.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::{Map, Value};

use memstead_git_branch::mem_repo_config::branch_ref_for_mem_at_gitdir;

use crate::CliError;
use crate::output::{ExitKind, print_json};
use crate::setup::{CliContext, find_workspace_root, workspace_not_initialised_error};
use memstead_engine::workspace_config_edit::{
    self, CrossLinkTarget, WorkspaceEditError, WorkspaceEditWarning,
};

/// Subcommands under `memstead workspace`.
#[derive(Subcommand, Debug)]
pub enum WorkspaceAction {
    /// Emit a JSON document describing the workspace's mems, the
    /// schema each is pinned to, and per-mem opaque snapshot tokens.
    /// Output is always JSON (the global `--json` is a no-op here).
    Dump(DumpArgs),

    /// Render the active workspace configuration: mem-management
    /// allowlists, cross-mem permissions, mutation policy, plugin
    /// sections. Markdown by default; `--json` emits a structured
    /// document. Counterpart to the `allow-create / grant-cross-link /
    /// set-mutations` write surface — read what those commands have
    /// composed.
    Show(ShowArgs),

    /// Add a `[[mem_management.create]]` allowlist rule. Pattern
    /// uses gitignore-style globs (`*` does not cross `/`, `**`
    /// matches zero-or-more segments). Schemas pin which schemas the
    /// agent may bring into existence under this namespace; `--schema *`
    /// allows any schema. Order: appended (lowest priority) by
    /// default; `--before <pattern>` lifts it above the named pattern.
    #[command(name = "allow-create")]
    AllowCreate(AllowCreateArgs),

    /// Remove a `[[mem_management.create]]` rule by pattern.
    #[command(name = "revoke-create")]
    RevokeCreate(PatternArg),

    /// Add a `[[mem_management.delete]]` allowlist rule.
    #[command(name = "allow-delete")]
    AllowDelete(PatternArg),

    /// Remove a `[[mem_management.delete]]` rule by pattern.
    #[command(name = "revoke-delete")]
    RevokeDelete(PatternArg),

    /// Grant a `[cross_mem_links]` permission: `<from>` may write
    /// edges into `<to>`. `<to>` is `*` for the wildcard shape or a
    /// mem name for the allowlist shape. Mixing the two for one
    /// `from`-mem is rejected.
    #[command(name = "grant-cross-link")]
    GrantCrossLink(CrossLinkArgs),

    /// Revoke a `[cross_mem_links]` permission. Removes the named
    /// target from the allowlist; drops the `from`-key entirely when
    /// the allowlist becomes empty. `*` revokes the wildcard shape.
    #[command(name = "revoke-cross-link")]
    RevokeCrossLink(CrossLinkArgs),

    /// Set a `[mutations]` field. Today exposes `--require-notes`
    /// only; additional keys land additively.
    #[command(name = "set-mutations")]
    SetMutations(SetMutationsArgs),
}

/// `memstead workspace dump` arguments. `--json` is the root-level global
/// flag (the dump is always emitted as JSON regardless).
#[derive(Args, Debug)]
pub struct DumpArgs {}

/// `memstead workspace show` arguments. `--json` is the root-level global
/// flag (Markdown by default, JSON when set).
#[derive(Args, Debug)]
pub struct ShowArgs {}

/// Args for `allow-create`.
#[derive(Args, Debug)]
pub struct AllowCreateArgs {
    /// Glob pattern (gitignore semantics) the rule matches against
    /// the lifecycle candidate `<path>/<name>` (or `<name>` for
    /// flat-layout mems).
    pub pattern: String,

    /// Schema pins the rule permits. Repeat or pass as a single
    /// comma-separated value. `*` is the any-schema escape.
    #[arg(long, required = true, value_delimiter = ',')]
    pub schema: Vec<String>,

    /// Cross-mem permission conferred on every mem matching this
    /// rule. Rule-derived and evaluated lazily at relate time — not
    /// written into `[cross_mem_links]`; `workspace show` and
    /// `memstead_overview` surface it under the rule. Repeat or pass as a
    /// single comma-separated value; `*` for wildcard.
    #[arg(long, value_delimiter = ',')]
    pub cross_link: Vec<String>,

    /// Insert this rule before the named pattern (lifts it above the
    /// target in the first-match-wins order). Omit to append at the
    /// lowest priority.
    #[arg(long)]
    pub before: Option<String>,
}

/// Single-pattern args for `revoke-create / allow-delete / revoke-delete`.
#[derive(Args, Debug)]
pub struct PatternArg {
    /// Pattern identifying the rule.
    pub pattern: String,
}

/// Args for `grant-cross-link / revoke-cross-link`.
#[derive(Args, Debug)]
pub struct CrossLinkArgs {
    /// Source mem (the `from` side of the permission).
    pub from: String,
    /// Target mem or `*` for the wildcard shape.
    pub to: String,
}

/// Args for `set-mutations`.
#[derive(Args, Debug)]
pub struct SetMutationsArgs {
    /// Toggle `[mutations] require_notes`. When set, mutations without
    /// a `note` field surface a `note_missing` warning (the mutation
    /// still lands — provenance is best-effort).
    #[arg(long, value_name = "BOOL", value_parser = clap::value_parser!(bool))]
    pub require_notes: Option<bool>,
}

pub fn run(ctx: &CliContext, action: WorkspaceAction) -> anyhow::Result<()> {
    match action {
        WorkspaceAction::Dump(args) => dump(ctx, args),
        WorkspaceAction::Show(args) => show(ctx, args),
        WorkspaceAction::AllowCreate(args) => allow_create(ctx, args),
        WorkspaceAction::RevokeCreate(args) => revoke_create(ctx, args),
        WorkspaceAction::AllowDelete(args) => allow_delete(ctx, args),
        WorkspaceAction::RevokeDelete(args) => revoke_delete(ctx, args),
        WorkspaceAction::GrantCrossLink(args) => grant_cross_link(ctx, args),
        WorkspaceAction::RevokeCrossLink(args) => revoke_cross_link(ctx, args),
        WorkspaceAction::SetMutations(args) => set_mutations(ctx, args),
    }
}

/// Walk up from cwd to the workspace root that carries
/// `.memstead/workspace.toml`. Returns a `CliError` shaped to match the
/// rest of the workspace-not-initialised paths in `setup.rs`.
fn require_workspace_root() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().map_err(|e| CliError {
        code: crate::INTERNAL_CODE,
        kind: ExitKind::Generic,
        message: format!("could not determine current directory: {e}"),
        details: None,
    })?;
    find_workspace_root(&cwd).ok_or_else(|| {
        workspace_not_initialised_error(
            "No workspace found. Run from a directory containing `.memstead/workspace.toml` (run `memstead mem-repo init` or `memstead init` to bootstrap).",
        )
        .into()
    })
}

/// Convert a `WorkspaceEditError` into a typed CLI error. The
/// stable `code` from the writer is lifted into the `--json` envelope.
/// Idempotency variants (`RuleAlreadyPresent`, `RuleNotFoundNoop`,
/// `GrantAlreadyPresent`, `GrantNotFound`) do not surface here — they
/// ride on the success path as `WorkspaceEditWarning` instances,
/// rendered via [`emit_warnings`].
fn lift_edit_error(err: WorkspaceEditError) -> CliError {
    let code = err.code();
    let message = err.to_string();
    // Match by ref so `err`'s fields can ride into a structured `details`
    // payload without consuming the value before `to_string()`/`code()`.
    let (kind, details) = match &err {
        WorkspaceEditError::WorkspaceNotInitialised { .. } => (ExitKind::Generic, None),
        WorkspaceEditError::InvalidToml { .. } => (ExitKind::Validation, None),
        WorkspaceEditError::BeforePatternNotFound { .. } => (ExitKind::NotFound, None),
        WorkspaceEditError::CrossLinkConflict { .. } => (ExitKind::Validation, None),
        WorkspaceEditError::RuleExistsSchemasDiffer {
            section,
            pattern,
            stored,
            requested,
        } => (
            ExitKind::Validation,
            Some(serde_json::json!({
                "section": section,
                "pattern": pattern,
                "stored_schemas": stored,
                "requested_schemas": requested,
                "recovery": format!(
                    "revoke-create {pattern} then allow-create {pattern} --schema … with the new schemas"
                ),
            })),
        ),
        WorkspaceEditError::Io { .. } => (ExitKind::Generic, None),
    };
    // The writer's `code()` returns a `&'static str` already — promote
    // it into the CliError's typed `code` slot so the wire envelope
    // carries it verbatim rather than burying it under `details`.
    CliError {
        kind,
        code,
        message,
        details,
    }
}

/// Render idempotency warnings on stderr — used in `--json` mode so
/// the envelope's `{action, detail}` shape stays untouched. Markdown-
/// default mode embeds the warnings under a `## Warnings` block via
/// [`confirm_block`] instead, matching every other CLI mutation's
/// rendering shape.
///
/// The engine writers return `Ok(Vec<WorkspaceEditWarning>)` for
/// idempotency cases so scripts and agents can retry without branching
/// on prior state.
fn emit_warnings_stderr(warnings: &[WorkspaceEditWarning]) {
    for w in warnings {
        eprintln!("warning [{}]: {}", w.code(), w);
    }
}

fn parse_schemas(schemas: &[String]) -> Vec<String> {
    // `--schema` is `Vec<String>` from clap with `value_delimiter = ','`,
    // so the comma-splitting is already handled. This helper exists so
    // future per-pin validation can land in one place.
    schemas.to_vec()
}

fn parse_cross_links(targets: &[String]) -> Vec<CrossLinkTarget> {
    targets.iter().map(|t| CrossLinkTarget::parse(t)).collect()
}

/// Render a workspace-mutation response. The `--json` shape is the
/// `{action, detail}` envelope; markdown-default
/// mode emits a markdown block (heading + bullet list + optional
/// `## Warnings` block) matching the shape every other CLI mutation
/// (`mem init`, `relate`, `create`, …) uses.
///
/// `heading` is the top-level title (e.g. `"Workspace allow-create
/// rule \`scratch-*\`"`). `bullets` is a list of pre-formatted bullet
/// strings (already including the leading `"- "` is NOT required —
/// the helper adds the dash). `warnings` mirrors the engine's
/// idempotency notices; in `--json` mode they ride out on stderr to
/// keep the envelope shape untouched.
fn confirm_block(
    ctx: &CliContext,
    action: &str,
    detail: serde_json::Value,
    heading: &str,
    bullets: Vec<String>,
    warnings: &[WorkspaceEditWarning],
) -> anyhow::Result<()> {
    if ctx.json {
        emit_warnings_stderr(warnings);
        let payload = serde_json::json!({ "action": action, "detail": detail });
        return print_json(&payload);
    }
    let mut lines: Vec<String> = Vec::with_capacity(2 + bullets.len() + 2 * warnings.len());
    lines.push(format!("# {heading}"));
    lines.push(String::new());
    for b in bullets {
        lines.push(format!("- {b}"));
    }
    if !warnings.is_empty() {
        lines.push(String::new());
        lines.push("## Warnings".to_string());
        lines.push(String::new());
        for w in warnings {
            lines.push(format!("- **{}**: {}", w.code(), w));
        }
    }
    crate::output::print_markdown(&lines.join("\n"));
    Ok(())
}

/// Render a list of strings as a bracketed comma-joined inline list
/// (e.g. `[a, b, c]`). Empty list renders as `(none)` — the markdown
/// renderer uses this for both the `schemas` and `cross_links` bullet
/// lines so an operator distinguishes "no entries" from "the list is
/// the wildcard `*`" (which renders as `*`).
fn render_list_inline(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        format!("[{}]", items.join(", "))
    }
}

fn allow_create(ctx: &CliContext, args: AllowCreateArgs) -> anyhow::Result<()> {
    let root = require_workspace_root()?;
    let schemas = parse_schemas(&args.schema);
    let cross_links = parse_cross_links(&args.cross_link);
    let cross_links_opt = if cross_links.is_empty() {
        None
    } else {
        Some(cross_links.as_slice())
    };
    let warnings = workspace_config_edit::add_create_rule(
        &root,
        &args.pattern,
        &schemas,
        cross_links_opt,
        args.before.as_deref(),
    )
    .map_err(lift_edit_error)?;
    let heading = format!("Workspace allow-create rule `{}`", args.pattern);
    let position = args
        .before
        .as_deref()
        .map(|p| format!("Position: before `{p}`"))
        .unwrap_or_else(|| "Position: appended (lowest priority)".to_string());
    let bullets = vec![
        format!("Pattern: `{}`", args.pattern),
        format!("Schemas: {}", render_list_inline(&schemas)),
        format!(
            "Default cross-links: {}",
            render_list_inline(&args.cross_link)
        ),
        position,
    ];
    confirm_block(
        ctx,
        "allow-create",
        serde_json::json!({
            "pattern": args.pattern,
            "schemas": schemas,
            "before": args.before,
            "cross_links": args.cross_link,
        }),
        &heading,
        bullets,
        &warnings,
    )
}

fn revoke_create(ctx: &CliContext, args: PatternArg) -> anyhow::Result<()> {
    let root = require_workspace_root()?;
    let warnings =
        workspace_config_edit::remove_create_rule(&root, &args.pattern).map_err(lift_edit_error)?;
    let heading = format!("Workspace revoke-create rule `{}`", args.pattern);
    let bullets = vec![format!("Pattern: `{}`", args.pattern)];
    confirm_block(
        ctx,
        "revoke-create",
        serde_json::json!({ "pattern": args.pattern }),
        &heading,
        bullets,
        &warnings,
    )
}

fn allow_delete(ctx: &CliContext, args: PatternArg) -> anyhow::Result<()> {
    let root = require_workspace_root()?;
    let warnings =
        workspace_config_edit::add_delete_rule(&root, &args.pattern).map_err(lift_edit_error)?;
    let heading = format!("Workspace allow-delete rule `{}`", args.pattern);
    let bullets = vec![format!("Pattern: `{}`", args.pattern)];
    confirm_block(
        ctx,
        "allow-delete",
        serde_json::json!({ "pattern": args.pattern }),
        &heading,
        bullets,
        &warnings,
    )
}

fn revoke_delete(ctx: &CliContext, args: PatternArg) -> anyhow::Result<()> {
    let root = require_workspace_root()?;
    let warnings =
        workspace_config_edit::remove_delete_rule(&root, &args.pattern).map_err(lift_edit_error)?;
    let heading = format!("Workspace revoke-delete rule `{}`", args.pattern);
    let bullets = vec![format!("Pattern: `{}`", args.pattern)];
    confirm_block(
        ctx,
        "revoke-delete",
        serde_json::json!({ "pattern": args.pattern }),
        &heading,
        bullets,
        &warnings,
    )
}

fn grant_cross_link(ctx: &CliContext, args: CrossLinkArgs) -> anyhow::Result<()> {
    let root = require_workspace_root()?;
    // Registered mems (any mount) drive the grant's target validation
    // in the shared engine policy-edit layer — same warnings the MCP
    // surface emits. Building the engine mirrors `workspace dump`.
    let known_mems: Vec<String> = {
        let engine = crate::setup::pro_engine(ctx)?;
        engine.mem_names().iter().map(|s| s.to_string()).collect()
    };
    let target = CrossLinkTarget::parse(&args.to);
    let warnings = workspace_config_edit::grant_cross_link(&root, &args.from, &target, &known_mems)
        .map_err(lift_edit_error)?;
    let heading = format!("Workspace grant-cross-link `{}` → `{}`", args.from, args.to);
    let bullets = vec![
        format!("From: `{}`", args.from),
        format!("To: `{}`", args.to),
    ];
    confirm_block(
        ctx,
        "grant-cross-link",
        serde_json::json!({ "from": args.from, "to": args.to }),
        &heading,
        bullets,
        &warnings,
    )
}

fn revoke_cross_link(ctx: &CliContext, args: CrossLinkArgs) -> anyhow::Result<()> {
    let root = require_workspace_root()?;
    let target = CrossLinkTarget::parse(&args.to);
    let warnings = workspace_config_edit::revoke_cross_link(&root, &args.from, &target)
        .map_err(lift_edit_error)?;
    let heading = format!(
        "Workspace revoke-cross-link `{}` → `{}`",
        args.from, args.to
    );
    let bullets = vec![
        format!("From: `{}`", args.from),
        format!("To: `{}`", args.to),
    ];
    confirm_block(
        ctx,
        "revoke-cross-link",
        serde_json::json!({ "from": args.from, "to": args.to }),
        &heading,
        bullets,
        &warnings,
    )
}

fn show(ctx: &CliContext, _args: ShowArgs) -> anyhow::Result<()> {
    use memstead_base::{FileWorkspaceStore, WorkspaceStoreAdapter};

    let root = require_workspace_root()?;
    let workspace = FileWorkspaceStore::new().load(&root).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "WORKSPACE_CONFIG_READ_FAILED",
            format!("workspace show: load `{}`: {e}", root.display()),
        )
    })?;

    let settings = &workspace.settings;
    let json_mode = ctx.json;
    if json_mode {
        let create_rules: Vec<serde_json::Value> = settings
            .mem_create_rules
            .iter()
            .map(|r| {
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "pattern".to_string(),
                    serde_json::Value::String(r.pattern.clone()),
                );
                obj.insert(
                    "schemas".to_string(),
                    serde_json::Value::Array(
                        r.schemas
                            .iter()
                            .map(|s| serde_json::Value::String(s.clone()))
                            .collect(),
                    ),
                );
                if let Some(cl) = &r.default_cross_links {
                    obj.insert(
                        "default_cross_links".to_string(),
                        cross_link_value_to_json(cl),
                    );
                }
                serde_json::Value::Object(obj)
            })
            .collect();
        let delete_rules: Vec<serde_json::Value> = settings
            .mem_delete_rules
            .iter()
            .map(|r| serde_json::json!({ "pattern": r.pattern }))
            .collect();
        let mut cross_links_obj = serde_json::Map::new();
        for (k, v) in &settings.cross_mem_links {
            cross_links_obj.insert(k.clone(), cross_link_value_to_json(v));
        }
        let mut mutations_obj = serde_json::Map::new();
        if let Some(rn) = settings.mutations.require_notes {
            mutations_obj.insert("require_notes".to_string(), serde_json::Value::Bool(rn));
        }
        let mut plugin_obj = serde_json::Map::new();
        for (k, v) in &settings.plugin {
            plugin_obj.insert(k.clone(), serde_json::Value::String(v.to_string()));
        }
        let document = serde_json::json!({
            "workspace_root": root.display().to_string(),
            "mem_management": {
                "create": create_rules,
                "delete": delete_rules,
            },
            "cross_mem_links": cross_links_obj,
            "mutations": mutations_obj,
            "plugin": plugin_obj,
        });
        return print_json(&document);
    }

    let mut lines = Vec::new();
    lines.push("# Workspace configuration".to_string());
    lines.push(String::new());
    lines.push(format!("- Root: `{}`", root.display()));
    lines.push(String::new());

    lines.push("## Mem management".to_string());
    lines.push(String::new());
    if settings.mem_create_rules.is_empty() {
        lines.push(
            "- `[[mem_management.create]]`: (none — no agent-driven mem creation allowed)"
                .to_string(),
        );
    } else {
        lines.push("- `[[mem_management.create]]`:".to_string());
        for r in &settings.mem_create_rules {
            let cross = match &r.default_cross_links {
                None => String::new(),
                Some(v) => format!(" → cross-links: {}", render_cross_link_value(v)),
            };
            lines.push(format!(
                "  - `{}` schemas=[{}]{cross}",
                r.pattern,
                r.schemas.join(", "),
            ));
        }
    }
    if settings.mem_delete_rules.is_empty() {
        lines.push("- `[[mem_management.delete]]`: (none)".to_string());
    } else {
        lines.push("- `[[mem_management.delete]]`:".to_string());
        for r in &settings.mem_delete_rules {
            lines.push(format!("  - `{}`", r.pattern));
        }
    }
    lines.push(String::new());

    lines.push("## Cross-mem links".to_string());
    lines.push(String::new());
    if settings.cross_mem_links.is_empty() {
        lines.push("- `[cross_mem_links]`: (none — default-deny)".to_string());
    } else {
        for (from, value) in &settings.cross_mem_links {
            lines.push(format!("- `{from}` → {}", render_cross_link_value(value)));
        }
    }
    lines.push(String::new());

    lines.push("## Mutations".to_string());
    lines.push(String::new());
    match settings.mutations.require_notes {
        Some(true) => lines.push("- `require_notes`: `true`".to_string()),
        Some(false) => lines.push("- `require_notes`: `false`".to_string()),
        None => lines.push("- `require_notes`: (unset — best-effort)".to_string()),
    }
    lines.push(String::new());

    if !settings.plugin.is_empty() {
        lines.push("## Plugin (opaque pass-through)".to_string());
        lines.push(String::new());
        let mut keys: Vec<&String> = settings.plugin.keys().collect();
        keys.sort();
        for k in keys {
            lines.push(format!(
                "- `[plugin.{k}]`: (operator-managed; CLI does not edit)"
            ));
        }
    }

    crate::output::print_markdown(&lines.join("\n"));
    Ok(())
}

fn cross_link_value_to_json(
    v: &memstead_schema::workspace_config::CrossLinkValue,
) -> serde_json::Value {
    use memstead_schema::workspace_config::CrossLinkValue;
    match v {
        CrossLinkValue::Wildcard => serde_json::Value::String("*".to_string()),
        CrossLinkValue::List(names) => serde_json::Value::Array(
            names
                .iter()
                .map(|n| serde_json::Value::String(n.clone()))
                .collect(),
        ),
    }
}

fn render_cross_link_value(v: &memstead_schema::workspace_config::CrossLinkValue) -> String {
    use memstead_schema::workspace_config::CrossLinkValue;
    match v {
        CrossLinkValue::Wildcard => "*".to_string(),
        CrossLinkValue::List(names) => format!("[{}]", names.join(", ")),
    }
}

fn set_mutations(ctx: &CliContext, args: SetMutationsArgs) -> anyhow::Result<()> {
    let root = require_workspace_root()?;
    if let Some(value) = args.require_notes {
        workspace_config_edit::set_mutation_require_notes(&root, value).map_err(lift_edit_error)?;
        let heading = "Workspace set-mutations".to_string();
        let bullets = vec![format!("`require_notes`: `{value}`")];
        confirm_block(
            ctx,
            "set-mutations",
            serde_json::json!({ "require_notes": value }),
            &heading,
            bullets,
            &[],
        )
    } else {
        Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            "set-mutations requires at least one of: --require-notes <bool>",
        )
        .into())
    }
}

/// Format string emitted into the `format` field of the dump.
///
/// Plugin gates on this exact value. Any structurally-breaking change
/// (renamed key, dropped key, changed value type) must bump to
/// `workspace-dump/v1` and the consumer's gate must be widened to
/// accept the new value as part of the same change set.
const DUMP_FORMAT: &str = "workspace-dump/v0";

#[derive(Serialize)]
struct DumpMem {
    name: String,
    /// Mount capability — `"writable"` or `"read_only"`. RO mounts
    /// have no gitdir / snapshot_token; consumers branch on this to
    /// know which conditional fields are populated.
    capability: &'static str,
    /// Schema pin as it appears in the mem config (`"software"` or
    /// `"software@1.0.0"`). `None` is preserved as JSON `null` so
    /// consumers can branch on the unset case without inferring it from
    /// an absent key. Serialized as `schema_ref` for consistency with
    /// `memstead_mem_create`'s response (where `schema_ref` is the short
    /// name string and `schema` is the inlined schema body).
    #[serde(rename = "schema_ref")]
    schema: Option<String>,
    /// One-line description from the mem config; `None` when unset.
    description: Option<String>,
    /// Opaque pass-through of `MemConfig.write_guidance` — the same
    /// `HashMap<String, Value>` shape the engine carries on disk and on
    /// the wire.
    ///
    /// Serialized snake_case (`write_guidance`), uniform with every
    /// neighbouring key in the dump envelope — even though the on-disk
    /// `MemConfig` carries it camelCase.
    write_guidance: Map<String, Value>,
    /// Opaque token that changes iff the mem content has changed
    /// since the previous dump. Consumers' only legal operation is
    /// byte-equality. Today the token is the per-mem content branch's
    /// fully-peeled HEAD oid for git-branch mounts; `None` for folder
    /// and archive mounts whose backends have no head-of-branch
    /// concept (the archive's bytes are immutable post-install; folder
    /// mems aren't change-tracked by the engine).
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_token: Option<String>,
    /// Verbatim pass-through of `MemConfig.sync_state` — the ingest
    /// layer's durable "last synced source state" baseline, keyed per
    /// `(ingest, facet)`. Each value is an opaque token the engine
    /// never interprets (git → commit id, graph → snapshot token,
    /// filesystem → a JSON-stringified stat digest). This is the read
    /// pipe the ingest loop diffs against to steer at the changed
    /// slice; writes route through `memstead mem set-sync-state`.
    /// Omitted from the wire when empty, like `write_guidance` —
    /// existing minimal configs don't gain an empty `{}`.
    #[serde(skip_serializing_if = "Map::is_empty")]
    sync_state: Map<String, Value>,
}

#[derive(Serialize)]
struct DumpSchema {
    /// Schema-level writing-guidance defaults. Always present (possibly
    /// empty) so consumers don't need a key-existence check.
    #[serde(rename = "default_writing_guidance")]
    default_writing_guidance: SchemaWritingGuidance,
}

#[derive(Serialize, Default)]
struct SchemaWritingGuidance {
    avoid: Option<String>,
    goal: Option<String>,
}

fn dump(_ctx: &CliContext, _args: DumpArgs) -> anyhow::Result<()> {
    let setup_ctx = CliContext {
        json: true,
        quiet: false,
    };
    // Engine init can fail for several reasons; `WORKSPACE_NOT_INITIALISED`
    // is the dominant one for cold-start usage and the only one the test
    // contract pins. Pre-fix the boot error was wrapped under a generic
    // INTERNAL string — wire envelope drifted from the typed code.
    let engine = crate::setup::pro_engine(&setup_ctx).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "WORKSPACE_NOT_INITIALISED",
            format!("workspace dump: could not initialize engine: {e}"),
        )
    })?;

    let mut mems: Vec<DumpMem> = Vec::new();
    let mut schemas: Map<String, Value> = Map::new();

    for (name, config) in engine.mem_configs_named() {
        // F24: branch on mount capability. Git-branch (writable)
        // mounts emit the gitdir-derived snapshot token; folder and
        // archive (RO) mounts omit the token entirely. Calling
        // `gitdir_for(name)` unconditionally would trip
        // `EngineError::Mem` for archive mounts, crashing the whole
        // dump with `MEM_ERROR` on the first RO mount encountered.
        let mount = engine.mount(name);
        let capability = match mount.map(|m| m.capability) {
            Some(memstead_base::MountCapability::ReadOnly) => "read_only",
            _ => "writable",
        };
        let storage = mount.map(|m| &m.storage);
        let snapshot_token: Option<String> = match storage {
            Some(memstead_base::MountStorage::GitBranch { .. }) => {
                let gitdir = engine.gitdir_for(name).map_err(|e| {
                    CliError::new(
                        ExitKind::Generic,
                        "MEM_ERROR",
                        format!("workspace dump: gitdir for mem '{name}': {e}"),
                    )
                })?;
                Some(read_branch_head_oid(&gitdir, name).map_err(|e| {
                    CliError::new(
                        ExitKind::Generic,
                        "MEM_ERROR",
                        format!("workspace dump: snapshot token for mem '{name}': {e}"),
                    )
                })?)
            }
            // Folder and Archive mounts have no head-of-branch — the
            // token is intentionally absent so `Option::is_none` skips
            // serialisation. Consumers branch on `capability` plus the
            // field's presence.
            _ => None,
        };

        let schema_pin = config
            .schema
            .as_ref()
            .map(|p| {
                serde_json::to_value(p)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
            })
            .unwrap_or(None);

        let mut write_guidance = Map::new();
        for (k, v) in &config.write_guidance {
            write_guidance.insert(k.clone(), v.clone());
        }

        // Sync-state tokens are opaque strings; surface them verbatim
        // as JSON string values. The consumer (ingest loop) owns their
        // interpretation per medium type.
        let mut sync_state = Map::new();
        for (k, v) in &config.sync_state {
            sync_state.insert(k.clone(), Value::String(v.clone()));
        }

        mems.push(DumpMem {
            name: name.to_string(),
            capability,
            schema: schema_pin.clone(),
            description: config.description.clone(),
            write_guidance,
            snapshot_token,
            sync_state,
        });

        // Record the schema body for this pin if we haven't seen it yet.
        if let Some(pin) = schema_pin
            && !schemas.contains_key(&pin)
            && let Some(schema) = engine.schema_for(name)
        {
            let dwg = schema
                .manifest
                .default_writing_guidance
                .as_ref()
                .map(|d| SchemaWritingGuidance {
                    avoid: d.avoid.clone(),
                    goal: d.goal.clone(),
                })
                .unwrap_or_default();
            let body = DumpSchema {
                default_writing_guidance: dwg,
            };
            schemas.insert(pin, serde_json::to_value(body)?);
        }
    }

    mems.sort_by(|a, b| a.name.cmp(&b.name));

    let workspace_root = std::env::current_dir()
        .ok()
        .and_then(|cwd| crate::setup::find_workspace_root(&cwd).map(|p| p.display().to_string()));

    let document = serde_json::json!({
        "format": DUMP_FORMAT,
        "workspace_root": workspace_root,
        "mems": mems,
        "schemas": schemas,
    });

    print_json(&document)?;
    Ok(())
}

/// Read the fully-peeled HEAD oid of the per-mem content branch as a
/// hex string. This is the dump's snapshot-token primitive — the value
/// changes iff a new commit lands on the mem's branch, which is iff
/// the mem content changed.
///
/// `mem_name` is the leaf; the helper walks `__MEMSTEAD:mems/` to find
/// the hierarchical branch ref (`refs/heads/<path>/<leaf>`) and reads
/// the oid from there.
fn read_branch_head_oid(gitdir: &Path, mem_name: &str) -> Result<String, String> {
    if !gitdir.is_dir() {
        return Err(format!("gitdir not found at {}", gitdir.display()));
    }
    let repo = gix::open(gitdir).map_err(|e| format!("gix open: {e}"))?;
    let branch_ref = branch_ref_for_mem_at_gitdir(gitdir, mem_name);
    let reference = repo
        .find_reference(&branch_ref)
        .map_err(|e| format!("find ref {branch_ref}: {e}"))?;
    let oid = reference
        .into_fully_peeled_id()
        .map_err(|e| format!("peel {branch_ref}: {e}"))?;
    Ok(oid.to_string())
}
