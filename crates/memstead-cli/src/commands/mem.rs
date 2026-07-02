//! `memstead mem init` / `memstead mem delete` — pro-only mem-lifecycle
//! CLI front-ends.
//!
//! Both subcommands call the
//! engine in-process via `memstead_engine::mem_management::create_mem` /
//! `delete_mem`. An earlier design spawned `memstead-mcp --operator-mode`
//! as a child process and drove the matching MCP tool over JSON-RPC
//! — wire-format parity with the agent path was the intent,
//! but the CLI-as-MCP-consumer relationship cut against CLAUDE.md's
//! "CLI, MCP, UniFFI are siblings over the engine" posture. In-process
//! collapses CLI and MCP onto the same Rust call, with `operator_mode:
//! true` hardcoded at the call site so the engine bypasses the
//! `[[mem_management.create]]` / `[[mem_management.delete]]`
//! allowlists and the `MEM_REFERENCED_BY_POLICY` safeguard for these
//! two operator-tool surfaces (matching the spirit of the
//! transport-establishes-posture rule).
//!
//! Outer-repo gitignore: a CLI-only concern. The shared helper in
//! [`crate::outer_gitignore`] walks upward from the workspace root
//! looking for an enclosing `.git/`, then idempotently appends the
//! workspace path (or `mem-repo/` inside it) to the outer repo's
//! `.gitignore`. Refused for `$HOME` and disabled by `--no-gitignore`.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::outer_gitignore::{OuterRepoOutcome, apply_outer_gitignore};
use crate::CliError;
use crate::output::ExitKind;
use crate::setup::{CliContext, CliEngine, find_workspace_root};
use memstead_engine::mem_management::{
    self, MemCreateParams, MemCreateResponse, MemDeleteParams, MemDeleteResponse,
};

/// Subcommands under `memstead mem`.
#[derive(Subcommand, Debug)]
pub enum MemAction {
    /// Register a new mem via the engine's mem-management
    /// orchestrator.
    Init(InitArgs),
    /// Router-only removal — unregisters the mem from the workspace
    /// but leaves its stored content in place for archive workflows.
    /// Cross-mem grants pointing at the unregistered mem stay valid
    /// (the data they rely on survives); a follow-up `memstead mem init
    /// <same name>` re-attaches against the preserved storage. Refuses
    /// with `MEM_HAS_INCOMING_REFS` when entities in other mems still
    /// link into this one — remove those incoming cross-mem references
    /// first (mirrors `mem delete`'s precondition).
    Unregister(UnregisterArgs),
    /// Storage-destroying removal — unregisters the mem AND deletes
    /// its stored content. Refuses with `MEM_REFERENCED_BY_POLICY`
    /// when any other writable mem has a `cross_mem_links` grant
    /// pointing at the target (revoke the grant first). For router-only
    /// removal that keeps the storage, use `memstead mem unregister`.
    Delete(DeleteArgs),
    /// Update a mem's `version` field. The version is consumed by
    /// `memstead export --format mem` to stamp the archive filename and
    /// the `.mem` archive's published config. `version` is seeded at
    /// init (`0.1.0`); bump via this command before publishing.
    #[command(name = "set-version")]
    SetVersion(SetVersionArgs),
    /// Set a mem's schema pin — the integrity-driven schema-migration
    /// trigger. Already-integral mems switch immediately; otherwise
    /// the mem enters dual-pin migration (writes validate against
    /// the target) and the response lists the non-integral entities.
    /// Re-issue after repairing to complete the switch.
    #[command(name = "set-schema")]
    SetSchema(SetSchemaArgs),
    /// Set a mem's one-line `description` — embedded in `.mem` archive
    /// exports and surfaced on the registry card at publish time. An
    /// empty string clears the field. Set it before `memstead export` /
    /// `memstead publish` so the shared archive carries its card text.
    #[command(name = "set-description")]
    SetDescription(SetDescriptionArgs),
    /// Set (or clear) one opaque sync-state token in a mem's config —
    /// the ingest layer's durable "last synced source state" baseline.
    /// `<KEY>` and `<TOKEN>` are opaque to the engine (the ingest layer
    /// keys per `(ingest, facet)` and owns the token's meaning). An
    /// empty `<TOKEN>` clears the key. Written into the per-mem config
    /// and surfaced verbatim on `memstead workspace dump`.
    #[command(name = "set-sync-state")]
    SetSyncState(SetSyncStateArgs),
    /// Enumerate every mounted mem in the workspace with its
    /// schema pin, version, entity count, and capability (writable
    /// vs read-only). Markdown by default; pass `--json` (root flag)
    /// for the structured envelope.
    List(ListArgs),
}

/// `memstead mem list` — no positional args. The verb itself is the
/// signal; `--json` (root-level) toggles the output shape.
#[derive(Args, Debug)]
pub struct ListArgs {}

/// `memstead mem set-version <NAME> <VERSION>` arguments.
#[derive(Args, Debug)]
pub struct SetVersionArgs {
    /// Mem name (the leaf-folder identifier the engine assigned at
    /// init time). Must already be registered in the workspace.
    pub name: String,

    /// New semver version (e.g. `0.2.0`, `1.0.0-beta.1`). Malformed
    /// values refuse with `INVALID_INPUT`. The engine bypasses the
    /// mem-create allowlist for this surface — set-version is
    /// gate-free.
    pub version: String,

    /// Optional provenance note (≤280 chars) recorded on the
    /// version-bump commit body, like the other commit-producing
    /// mem-lifecycle commands. When the workspace sets
    /// `require_notes`, omitting it rides a non-blocking `NOTE_MISSING`
    /// warning (the bump still lands).
    #[arg(long)]
    pub note: Option<String>,
}

/// `memstead mem set-description <NAME> <DESCRIPTION>` arguments.
#[derive(Args, Debug)]
pub struct SetDescriptionArgs {
    /// Mem name (must be registered in the workspace).
    pub name: String,

    /// One-line description of the mem — what a registry visitor (or
    /// an agent browsing the catalogue) should know before installing.
    /// An empty string clears the field.
    pub description: String,

    /// Optional provenance note (≤280 chars) recorded on the commit
    /// body, like the other commit-producing mem-lifecycle commands.
    #[arg(long)]
    pub note: Option<String>,
}

/// `memstead mem set-sync-state <NAME> <KEY> <TOKEN>` arguments.
#[derive(Args, Debug)]
pub struct SetSyncStateArgs {
    /// Mem name (must be registered in the workspace).
    pub name: String,

    /// Opaque sync-state key. The ingest layer keys per `(ingest,
    /// facet)`, conventionally `"<ingest>/<facet>"`, but the engine
    /// treats it as an arbitrary string.
    pub key: String,

    /// Opaque token recording the source state last synced under
    /// `<KEY>` (git → commit id, graph → snapshot token, filesystem →
    /// a JSON-stringified stat digest). An **empty** value clears the
    /// key. The engine never parses it.
    pub token: String,

    /// Optional provenance note (≤280 chars) recorded on the commit
    /// body, like the other commit-producing mem-lifecycle commands.
    #[arg(long)]
    pub note: Option<String>,
}

/// `memstead mem set-schema <NAME> <SCHEMA>` arguments.
#[derive(Args, Debug)]
pub struct SetSchemaArgs {
    /// Mem name (must be registered in the workspace).
    pub name: String,

    /// Target schema ref, exact `name@x.y.z`. Must resolve against
    /// the loaded schema catalogue; unresolvable refs refuse with
    /// `SCHEMA_NOT_FOUND`, malformed refs with `INVALID_INPUT`.
    pub schema: String,
}

/// `memstead mem init <path>` arguments.
///
/// `--vcs-shared` translates into the engine's `vcs` block;
/// `--no-gitignore` suppresses the outer-repo `.gitignore` append. The
/// `<path>` argument supplies the new mem's `location` (relative
/// to the workspace root) plus its `name` (basename of the path); a
/// slashed `<a>/<b>` form additionally derives `--org-path a` so
/// `memstead mem init a/b` and `memstead mem init b --org-path a` produce
/// identical engine calls. Cross-mem edge authorization is
/// workspace-level policy (`[cross_mem_links]` in `.memstead/workspace.toml`); the
/// previous `--belongs-to` flag is gone.
#[derive(Args, Debug)]
pub struct InitArgs {
    /// Mem name — the full hierarchical identifier (e.g. `foo` for
    /// a flat-layout mem, `team/sub-mem` for a hierarchical
    /// layout). The value flows through to the engine verbatim with no
    /// auto-split or composition step. Grammar:
    /// `[a-z0-9-]+(/[a-z0-9-]+)*` — lowercase ASCII letters, digits,
    /// hyphens; segments separated by `/`; no leading, trailing, or
    /// double slashes (validated engine-side; bad names return
    /// `INVALID_INPUT`).
    pub path: PathBuf,

    /// Schema pin (`name@x.y.z`) for the new mem. Defaults to
    /// `default@1.0.0` so the common case stays one argument.
    #[arg(long, default_value = "default@1.0.0")]
    pub schema: String,

    /// Pass a shared-gitdir `vcs` block to `memstead_mem_create`:
    /// `{ "gitdir": "../.git", "worktree": ".." }`. Without this flag the
    /// engine uses the default isolated layout.
    #[arg(long)]
    pub vcs_shared: bool,

    /// Skip outer-repo `.gitignore` auto-append. Useful when the user
    /// intends to track the workspace as a git submodule, or when the
    /// detection heuristic would pick the wrong outer repo.
    #[arg(long)]
    pub no_gitignore: bool,

    /// Optional provenance note recorded in the seed commit's body
    /// (≤280 chars). Forwarded as the MCP tool's `note` parameter.
    #[arg(long)]
    pub note: Option<String>,

    /// Adopt residual entities left by a prior `memstead mem unregister`
    /// at this mem's path instead of failing on detected residue.
    /// Default when the residue carries an `unregistered_at` tombstone
    /// (the deliberate unregister signal); pass `--reattach` explicitly
    /// to override for crash-residue you have verified is safe to adopt.
    /// Mutually exclusive with `--force-overwrite` and
    /// `--hard-cleanup-first`.
    #[arg(long, group = "recovery_action")]
    pub reattach: bool,

    /// Destroy residual storage at this mem's path and proceed with a
    /// fresh create. **Not yet implemented** — currently refuses with
    /// `INVALID_INPUT` pointing at `memstead mem delete <name>`. Mutually
    /// exclusive with `--reattach` and `--hard-cleanup-first`.
    #[arg(long = "force-overwrite", group = "recovery_action")]
    pub force_overwrite: bool,

    /// Refuse with `MEM_STORAGE_RESIDUE_DETECTED` instructing the
    /// caller to run `memstead mem delete <name>` first — a hard barrier
    /// that keeps residue cleanup a separate, named operation rather
    /// than destructive auto-recovery. Mutually exclusive with
    /// `--reattach` and `--force-overwrite`.
    #[arg(long = "hard-cleanup-first", group = "recovery_action")]
    pub hard_cleanup_first: bool,

    /// Bypass the workspace `[[mem_management.create]]` allowlist
    /// for this invocation. The CLI honours the allowlist by default
    /// (matching the MCP-surface posture); operator-mode is explicit
    /// opt-in. Also settable via the `MEMSTEAD_OPERATOR_MODE=1` env var for
    /// script convenience; the flag wins when both are set. Use this
    /// when the CLI invocation is the operator administering the
    /// workspace itself (initial scaffold, recovery flows) rather than
    /// scripted/agent usage.
    #[arg(long = "operator-mode")]
    pub operator_mode: bool,

    /// Optional per-instance writing guidance as a JSON object, written
    /// verbatim into the new mem's config `writeGuidance` map — e.g.
    /// `--write-guidance '{"phase_context":"early design","stack":"Rust"}'`.
    /// Opaque to the engine (schema-strictness D8 — the keys are
    /// client-owned vocabulary); a wrapper that read the schema
    /// package's `mem-template.json` fills the instance keys. Omit to
    /// seed no guidance. Must be a JSON object; anything else refuses
    /// with `INVALID_INPUT`.
    #[arg(long = "write-guidance")]
    pub write_guidance: Option<String>,
}

/// `memstead mem delete <name>` arguments — full destruction. The
/// CLI honours the workspace `[[mem_management.delete]]`
/// allowlist by default; pass `--operator-mode` or set
/// `MEMSTEAD_OPERATOR_MODE=1` to skip the allowlist. The
/// `MEM_REFERENCED_BY_POLICY` and `MEM_HAS_INCOMING_REFS`
/// safeguards always fire regardless of operator-mode. The verb
/// uniquely identifies the storage-destroying intent — use
/// `memstead mem unregister` for router-only removal.
///
/// On success delete scrubs only the now-dangling
/// `[cross_mem_links]` grants naming this mem on either side
/// (reported in the `## Allowlist entries scrubbed` block of the
/// response) — those reference the gone instance and would otherwise
/// dangle. The workspace's `[[mem_management.create]]` /
/// `[[mem_management.delete]]` allowlist rules are PRESERVED, exact
/// name and glob alike: they are forward-looking permissions for the
/// name, not references to the instance. Re-creating a mem of the
/// same name afterward needs no fresh `allow-create` / `allow-delete`
/// grant.
#[derive(Args, Debug)]
pub struct DeleteArgs {
    /// Name of the mem to destroy.
    pub name: String,

    /// Optional provenance note (≤280 chars). Captured on the engine
    /// trace surface; surfaces via the outer-repo Stop hook. No
    /// per-mem commit is produced by delete.
    #[arg(long)]
    pub note: Option<String>,

    /// Bypass the workspace `[[mem_management.delete]]` allowlist
    /// for this invocation. See `InitArgs::operator_mode` for the
    /// full design rationale. Also settable via `MEMSTEAD_OPERATOR_MODE=1`.
    #[arg(long = "operator-mode")]
    pub operator_mode: bool,
}

/// `memstead mem unregister <name>` arguments — router-only removal,
/// storage preserved. The CLI honours the workspace
/// `[[mem_management.delete]]` allowlist by default; pass
/// `--operator-mode` or set `MEMSTEAD_OPERATOR_MODE=1` to skip the
/// allowlist. The `MEM_REFERENCED_BY_POLICY` safeguard does not
/// apply to unregister (storage is preserved), so unregistering a
/// mem with cross-mem grants pointing at it succeeds without
/// refusing — the data the grants rely on survives.
///
/// Refuses with `MEM_HAS_INCOMING_REFS` when an entity in another
/// Write-Mem still carries a graph edge into this mem (`details.referrers`
/// names each `{from_id, rel_types, mem}`) — remove those edges via
/// `memstead relate --remove` / `memstead update` first. This guard fires for
/// `unregister` just as it does for `delete`: the edge-graph axis is
/// independent of the storage-preservation choice, so a gentle
/// removal that left dangling cross-mem edges would be just as broken.
#[derive(Args, Debug)]
pub struct UnregisterArgs {
    /// Name of the mem to unregister.
    pub name: String,

    /// Optional provenance note (≤280 chars). Captured on the engine
    /// trace surface; surfaces via the outer-repo Stop hook.
    #[arg(long)]
    pub note: Option<String>,

    /// Bypass the workspace `[[mem_management.delete]]` allowlist
    /// for this invocation. See `InitArgs::operator_mode` for the
    /// full design rationale. Also settable via `MEMSTEAD_OPERATOR_MODE=1`.
    #[arg(long = "operator-mode")]
    pub operator_mode: bool,
}

pub fn run(ctx: &CliContext, args: InitArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().map_err(|e| {
        generic_error(format!("determine current directory: {e}"))
    })?;

    // Locate the workspace via the post-rebuild marker
    // (`.memstead/workspace.toml`). The presence of this file is the
    // engine's own boot precondition — `memstead-mcp` walks for it too.
    let workspace_root = find_workspace_root(&cwd).ok_or_else(|| {
        validation_error(format!(
            "no workspace found above {}. Run `memstead mem-repo init` first or \
             change directory into an existing workspace.",
            cwd.display(),
        ))
    })?;

    // Hierarchical paths are first-class mem identifiers. The CLI forwards
    // the `<PATH>` argument verbatim as `params.name` (`team/sub-mem`
    // or just `sub-mem` — the engine's mem-name grammar
    // validates the shape). There is no `--org-path` flag or path-vs-name
    // auto-split — the value flows through unchanged.
    let mem_name = args
        .path
        .to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| {
            invalid_input_error(format!(
                "mem name {:?} is not valid UTF-8 — mem names must be ASCII \
                 (lowercase letters / digits / hyphens) optionally segmented by '/'.",
                args.path.display(),
            ))
        })?;
    let location = mem_name.clone();

    let schema_ref: memstead_schema::SchemaRef = args.schema.parse().map_err(|e| {
        invalid_input_error(format!("invalid schema ref {:?}: {e}", args.schema))
    })?;
    let vcs_config = if args.vcs_shared {
        Some(memstead_schema::VcsConfig {
            gitdir: "../.git".to_string(),
            worktree: "..".to_string(),
        })
    } else {
        None
    };
    let write_guidance = match &args.write_guidance {
        None => std::collections::HashMap::new(),
        Some(raw) => serde_json::from_str::<
            std::collections::HashMap<String, serde_json::Value>,
        >(raw)
        .map_err(|e| {
            invalid_input_error(format!("--write-guidance must be a JSON object: {e}"))
        })?,
    };
    let params = MemCreateParams {
        name: mem_name.clone(),
        location: PathBuf::from(&location),
        schema_ref,
        vcs: vcs_config,
        note: args.note.clone(),
        write_guidance,
        // The workspace `[[mem_management.create]]`
        // allowlist applies to CLI calls by default; the operator
        // opts into bypass explicitly via `--operator-mode` (flag
        // wins) or `MEMSTEAD_OPERATOR_MODE=1` (env-var fallback).
        operator_mode: resolve_operator_mode(args.operator_mode),
        recovery: recovery_from_flags(
            args.reattach,
            args.force_overwrite,
            args.hard_cleanup_first,
        ),
    };

    let mut engine = match ctx.cli_engine()? {
        CliEngine::MemRepo(e) => e,
        CliEngine::Filesystem(_) => {
            return Err(validation_error(format!(
                "`memstead mem init` requires a mem-repo workspace; the workspace at {} is filesystem-shaped. Use `memstead mem-repo init` first to migrate.",
                workspace_root.display(),
            )));
        }
    };
    let response = mem_management::create_mem(&mut engine, params)
        .map_err(pro_engine_err_to_cli)?;
    if ctx.json {
        crate::output::print_json(&serde_json::json!({
            "name": response.name,
            "location": response.location,
            "schema_ref": response.schema_ref.to_string(),
            "seed_commit_sha": response.seed_commit_sha,
            // The reattach branch surfaces `MEM_REATTACHED_AFTER_UNREGISTER`
            // through the response envelope rather than dropping it on
            // the floor. Fresh-create ships an empty array.
            "warnings": response
                .warnings
                .iter()
                .map(|w| serde_json::json!({"code": w.code(), "message": w.message()}))
                .collect::<Vec<_>>(),
        }))?;
    } else {
        crate::output::print_markdown(&render_mem_create_markdown(&response));
    }

    // Outer-repo gitignore handling. Append `mem-repo/` (the post-cutover
    // gitignore target — every mem's content lives inside that one
    // directory) to the outer repo's `.gitignore`. Idempotent on re-run;
    // refuses when the outer is `$HOME`.
    if !args.no_gitignore {
        let mem_repo_path = workspace_root.join("mem-repo");
        let walk_start = workspace_root
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| workspace_root.clone());
        // Outer-repo provenance is human-facing context, not part of the
        // structured result. It goes to stderr — never stdout — so a `--json`
        // caller's stdout stays exactly one JSON document (the contract
        // `--help` advertises and steers callers to pipe through `jq`). A
        // human still sees it on the terminal in normal runs; `--quiet`
        // suppresses it, the first time this site consults the flag.
        match apply_outer_gitignore(&walk_start, &mem_repo_path)? {
            OuterRepoOutcome::Appended { outer_root, rel } => {
                if !ctx.quiet {
                    eprintln!(
                        "  outer:    {} — added `{}` to .gitignore",
                        outer_root.display(),
                        rel,
                    );
                }
            }
            OuterRepoOutcome::AlreadyIgnored { outer_root, rel } => {
                if !ctx.quiet {
                    eprintln!(
                        "  outer:    {} — `{}` already in .gitignore, no change",
                        outer_root.display(),
                        rel,
                    );
                }
            }
            OuterRepoOutcome::NoOuter | OuterRepoOutcome::Skipped => {}
        }
    }

    // Client-side mem-template consumption: when the operator did not
    // supply --write-guidance, surface the resolved schema's
    // mem-template instance keys so they know what to fill. The engine
    // treats `writeGuidance` opaquely — filling is the operator's job.
    if let Some(note) =
        mem_template_guidance_note(&response.schema_ref, args.write_guidance.is_some())
        && !ctx.quiet
    {
        eprintln!("  template: {note}");
    }

    Ok(())
}

/// When the operator did not supply `--write-guidance`, surface the
/// resolved (built-in) schema's `mem-template.json` instance guidance
/// keys so they know what to fill. Returns the operator notice, or
/// `None` when there is nothing to surface — guidance was already given,
/// the schema ships no template, or its template carries no guidance.
/// Reads only built-in templates; an installed/authored package's
/// template is a follow-up.
fn mem_template_guidance_note(
    schema_ref: &memstead_schema::SchemaRef,
    guidance_given: bool,
) -> Option<String> {
    if guidance_given {
        return None;
    }
    let template = memstead_schema::builtins::builtin_mem_template(&schema_ref.name)?;
    let wg = template.get("writeGuidance")?.as_object()?;
    if wg.is_empty() {
        return None;
    }
    let keys: Vec<&str> = wg.keys().map(String::as_str).collect();
    let first = keys.first().copied().unwrap_or("key");
    Some(format!(
        "schema {schema_ref} ships a mem-template with instance guidance key(s) [{}] — \
         the mem was created without guidance. Re-run with \
         --write-guidance '{{\"{first}\": \"…\"}}' (or edit the mem config) to fill them.",
        keys.join(", "),
    ))
}

pub fn run_delete(ctx: &CliContext, args: DeleteArgs) -> anyhow::Result<()> {
    run_delete_inner(
        ctx,
        args.name,
        args.note,
        /* delete_files */ true,
        "delete",
        resolve_operator_mode(args.operator_mode),
    )
}

pub fn run_unregister(ctx: &CliContext, args: UnregisterArgs) -> anyhow::Result<()> {
    run_delete_inner(
        ctx,
        args.name,
        args.note,
        /* delete_files */ false,
        "unregister",
        resolve_operator_mode(args.operator_mode),
    )
}

/// Resolve the effective operator-mode for a CLI invocation. The
/// workspace allowlist applies by default; the operator opts into
/// bypass via `--operator-mode` (highest precedence) or the
/// `MEMSTEAD_OPERATOR_MODE` env var. The env-var accepts `1`, `true`, `yes`
/// (case-insensitive); any other value is treated as unset.
fn resolve_operator_mode(flag: bool) -> bool {
    if flag {
        return true;
    }
    match std::env::var("MEMSTEAD_OPERATOR_MODE") {
        Ok(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => false,
    }
}

fn run_delete_inner(
    ctx: &CliContext,
    name: String,
    note: Option<String>,
    delete_files: bool,
    verb: &str,
    operator_mode: bool,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().map_err(|e| {
        generic_error(format!("determine current directory: {e}"))
    })?;
    let workspace_root = find_workspace_root(&cwd).ok_or_else(|| {
        validation_error(format!(
            "no workspace found above {}. `memstead mem {verb}` must run \
             inside a configured workspace.",
            cwd.display(),
        ))
    })?;

    let params = MemDeleteParams {
        name: name.clone(),
        delete_files,
        note: note.clone(),
        operator_mode,
    };
    let mut engine = match ctx.cli_engine()? {
        CliEngine::MemRepo(e) => e,
        CliEngine::Filesystem(_) => {
            return Err(validation_error(format!(
                "`memstead mem {verb}` requires a mem-repo workspace; the workspace at {} is filesystem-shaped.",
                workspace_root.display(),
            )));
        }
    };
    let response = mem_management::delete_mem(&mut engine, params)
        .map_err(pro_engine_err_to_cli)?;
    if ctx.json {
        crate::output::print_json(&serde_json::json!({
            "name": response.name,
            "deleted_from_router": response.deleted_from_router,
            "files_deleted": response.files_deleted,
            "warnings": response
                .warnings
                .iter()
                .map(|w| serde_json::json!({"code": w.code(), "message": w.message()}))
                .collect::<Vec<_>>(),
            // Surface scrubbed `.memstead/workspace.toml` entries so the
            // agent sees every policy side effect in one round-trip.
            "allowlist_entries_removed": &response.allowlist_entries_removed,
        }))?;
    } else {
        crate::output::print_markdown(&render_mem_delete_markdown(&response, verb));
    }
    Ok(())
}

/// Render a successful `MemCreateResponse` as a CLI markdown block.
/// The CLI owns its own prose rather than echoing the MCP subprocess's
/// pre-rendered text channel.
fn render_mem_create_markdown(r: &MemCreateResponse) -> String {
    // The reattach
    // branch surfaces a `MEM_REATTACHED_AFTER_UNREGISTER` warning on
    // the response. Adjust the heading so an operator picking up an
    // empty `seed_commit_sha` plus the reattach warning learns the
    // branch tip kept its prior history rather than starting fresh.
    let reattached = r
        .warnings
        .iter()
        .any(|w| matches!(w, memstead_base::ops::WarningHint::MemReattachedAfterUnregister { .. }));
    let heading = if reattached {
        format!("# Mem `{}` reattached\n\n", r.name)
    } else {
        format!("# Mem `{}` created\n\n", r.name)
    };
    let mut out = heading;
    out.push_str(&format!("- Location: `{}`\n", r.location.display()));
    out.push_str(&format!("- Schema: `{}`\n", r.schema_ref));
    out.push_str(&format!("- Seed commit: `{}`\n", r.seed_commit_sha));
    if !r.warnings.is_empty() {
        out.push_str("\n## Warnings\n\n");
        for w in &r.warnings {
            out.push_str(&format!("- **{}**: {}\n", w.code(), w.message()));
        }
    }
    out
}

/// Render a successful `MemDeleteResponse` as a CLI markdown block.
/// `verb` is the CLI subcommand name (`"delete"` or `"unregister"`)
/// — drives the heading prose so the output matches the user's
/// invocation.
fn render_mem_delete_markdown(r: &MemDeleteResponse, verb: &str) -> String {
    let past_participle = match verb {
        "unregister" => "unregistered",
        _ => "deleted",
    };
    let mut out = format!("# Mem `{}` {past_participle}\n\n", r.name);
    out.push_str(&format!(
        "- Removed from router: {}\n",
        r.deleted_from_router,
    ));
    out.push_str(&format!("- Files deleted: {}\n", r.files_deleted));
    // Surface every scrubbed `.memstead/workspace.toml` entry so the
    // operator sees what the destructive delete just cleaned up.
    if !r.allowlist_entries_removed.is_empty() {
        out.push_str("\n## Allowlist entries scrubbed\n\n");
        for entry in &r.allowlist_entries_removed {
            match (&entry.pattern, &entry.from, &entry.to) {
                (Some(p), _, _) => {
                    out.push_str(&format!(
                        "- `[{}]` pattern `{p}`\n",
                        entry.table,
                    ));
                }
                (_, Some(from), Some(to)) => {
                    out.push_str(&format!(
                        "- `[{}]` `{from} → {to}`\n",
                        entry.table,
                    ));
                }
                _ => {
                    out.push_str(&format!("- `[{}]`\n", entry.table));
                }
            }
        }
    }
    if !r.warnings.is_empty() {
        out.push_str("\n## Warnings\n\n");
        for w in &r.warnings {
            out.push_str(&format!("- **{}**: {}\n", w.code(), w.message()));
        }
    }
    out
}

/// Lift a `ProEngineError` into a typed `CliError`. The lift sources
/// every field from the engine error directly — `err.code()` for the
/// wire token, `err.details()` for the structured payload,
/// `err.prose_render()` for the text message. Wrapped basis errors
/// delegate to [`crate::CliError::from_engine_op`] so the per-variant
/// exit-kind mapping (`NotFound` → exit 3, `HashMismatch` → exit 4,
/// validation → exit 5, generic → exit 1) is consumed in one place;
/// lifecycle variants (`MEM_PATH_NOT_ALLOWED`,
/// `MEM_SCHEMA_NOT_ALLOWED`, `MEM_REFERENCED_BY_POLICY`,
/// `INVALID_MEM_NAME`, `CONFIG_ERROR`, `MEM_STORAGE_RESIDUE_DETECTED`)
/// are user-recoverable validation refusals and land at exit 5.
///
/// Sourcing from the engine error directly means any new engine code
/// automatically reaches the CLI envelope without a hand-maintained
/// translation table to update.
fn pro_engine_err_to_cli(err: memstead_engine::ProEngineError) -> anyhow::Error {
    match err {
        memstead_engine::ProEngineError::Basis(inner) => {
            CliError::from_engine_op(inner).into()
        }
        lifecycle => {
            let code = lifecycle.code();
            let details = lifecycle.details();
            let message = lifecycle.prose_render();
            CliError {
                kind: ExitKind::Validation,
                code,
                message,
                details: Some(details),
            }
            .into()
        }
    }
}

/// `memstead mem set-version <NAME> <VERSION>` — bump the mem's
/// `version` field via the in-process engine, persisting through the
/// backend's `write_mem_config`. Unlike `init` / `delete`, this
/// surface doesn't spawn the MCP subprocess: set-version is gate-free
/// (no operator-mode bypass needed), so a direct engine call keeps
/// the implementation simpler and faster.
pub fn run_set_version(ctx: &CliContext, args: SetVersionArgs) -> anyhow::Result<()> {
    let new_version = semver::Version::parse(&args.version).map_err(|e| {
        invalid_input_error(format!(
            "version {:?} is not a valid semver: {e}",
            args.version,
        ))
    })?;

    let note = args.note.as_deref();
    let outcome = match ctx.cli_engine()? {
        crate::setup::CliEngine::MemRepo(mut engine) => engine
            .set_mem_version(&args.name, new_version, note)
            .map_err(crate::CliError::from_engine_op)?,
        crate::setup::CliEngine::Filesystem(mut engine) => engine
            .set_mem_version(&args.name, new_version, note)
            .map_err(crate::CliError::from_engine_op)?,
    };

    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let old = outcome
            .old_version
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "<none>".to_string());
        let warnings = if outcome.warnings.is_empty() {
            String::new()
        } else {
            let rendered: Vec<String> = outcome
                .warnings
                .iter()
                .map(ToString::to_string)
                .collect();
            format!("\n\n> warnings:\n> - {}", rendered.join("\n> - "))
        };
        crate::output::print_markdown(&format!(
            "# Mem `{}` version updated\n\n- Old version: {}\n- New version: {}{}",
            outcome.mem, old, outcome.new_version, warnings,
        ));
    }
    Ok(())
}

/// `memstead mem set-description <NAME> <DESCRIPTION>` — set or clear
/// the mem's one-line description via the in-process engine,
/// persisting through the backend's `write_mem_config`. Like
/// set-version, this surface is gate-free and calls the engine
/// directly. An empty DESCRIPTION clears the field.
pub fn run_set_description(ctx: &CliContext, args: SetDescriptionArgs) -> anyhow::Result<()> {
    let new_description = {
        let trimmed = args.description.trim();
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    };
    let note = args.note.as_deref();
    let outcome = match ctx.cli_engine()? {
        crate::setup::CliEngine::MemRepo(mut engine) => engine
            .set_mem_description(&args.name, new_description, note)
            .map_err(crate::CliError::from_engine_op)?,
        crate::setup::CliEngine::Filesystem(mut engine) => engine
            .set_mem_description(&args.name, new_description, note)
            .map_err(crate::CliError::from_engine_op)?,
    };

    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let old = outcome.old_description.as_deref().unwrap_or("<none>");
        let new = outcome.new_description.as_deref().unwrap_or("<cleared>");
        let warnings = if outcome.warnings.is_empty() {
            String::new()
        } else {
            let rendered: Vec<String> =
                outcome.warnings.iter().map(ToString::to_string).collect();
            format!("\n\n> warnings:\n> - {}", rendered.join("\n> - "))
        };
        crate::output::print_markdown(&format!(
            "# Mem `{}` description updated\n\n- Old: {}\n- New: {}{}",
            outcome.mem, old, new, warnings,
        ));
    }
    Ok(())
}

/// `memstead mem set-sync-state <NAME> <KEY> <TOKEN>` — set or clear
/// one opaque sync-state token in a mem's config via the in-process
/// engine, persisting through the backend's `write_mem_config`. Like
/// set-version, this surface is gate-free and calls the engine directly.
pub fn run_set_sync_state(ctx: &CliContext, args: SetSyncStateArgs) -> anyhow::Result<()> {
    let note = args.note.as_deref();
    let outcome = match ctx.cli_engine()? {
        crate::setup::CliEngine::MemRepo(mut engine) => engine
            .set_mem_sync_state(&args.name, &args.key, &args.token, note)
            .map_err(crate::CliError::from_engine_op)?,
        crate::setup::CliEngine::Filesystem(mut engine) => engine
            .set_mem_sync_state(&args.name, &args.key, &args.token, note)
            .map_err(crate::CliError::from_engine_op)?,
    };

    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let action = if outcome.removed {
            "cleared".to_string()
        } else if outcome.previous.is_some() {
            "overwrote".to_string()
        } else {
            "set".to_string()
        };
        let warnings = if outcome.warnings.is_empty() {
            String::new()
        } else {
            let rendered: Vec<String> =
                outcome.warnings.iter().map(ToString::to_string).collect();
            format!("\n\n> warnings:\n> - {}", rendered.join("\n> - "))
        };
        crate::output::print_markdown(&format!(
            "# Mem `{}` sync state {}\n\n- Key: `{}`{}",
            outcome.mem, action, outcome.key, warnings,
        ));
    }
    Ok(())
}

pub fn run_set_schema(ctx: &CliContext, args: SetSchemaArgs) -> anyhow::Result<()> {
    let target: memstead_schema::SchemaRef = args.schema.parse().map_err(|e| {
        invalid_input_error(format!("invalid schema ref {:?}: {e}", args.schema))
    })?;
    let outcome = match ctx.cli_engine()? {
        crate::setup::CliEngine::MemRepo(mut engine) => engine
            .set_mem_schema(&args.name, &target)
            .map_err(crate::CliError::from_engine_op)?,
        crate::setup::CliEngine::Filesystem(mut engine) => engine
            .set_mem_schema(&args.name, &target)
            .map_err(crate::CliError::from_engine_op)?,
    };
    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let findings = if outcome.findings.is_empty() {
            String::new()
        } else {
            let rendered: Vec<String> = outcome
                .findings
                .iter()
                .map(|f| format!("- {} — {}", f.id, f.code))
                .collect();
            format!("\n\n## Non-integral entities\n\n{}", rendered.join("\n"))
        };
        crate::output::print_markdown(&format!(
            "# Mem `{}` schema: {:?}\n\n- Pin: {}\n- Migration target: {}{}",
            outcome.mem,
            outcome.outcome,
            outcome.schema_pin,
            outcome.migration_target.as_deref().unwrap_or("<none>"),
            findings,
        ));
    }
    Ok(())
}

fn generic_error(msg: String) -> anyhow::Error {
    CliError {
        code: "MEM_ERROR",
        kind: ExitKind::Generic,
        message: msg,
        details: None,
    }
    .into()
}

fn validation_error(msg: String) -> anyhow::Error {
    CliError {
        code: "VALIDATION_FAILED",
        kind: ExitKind::Validation,
        message: msg,
        details: None,
    }
    .into()
}

fn invalid_input_error(msg: String) -> anyhow::Error {
    CliError {
        code: "INVALID_INPUT",
        kind: ExitKind::Validation,
        message: msg,
        details: None,
    }
    .into()
}

/// Bridge the three single-purpose CLI flags into a single
/// `RecoveryAction` enum value. clap's `group = "recovery_action"`
/// annotation on each flag enforces the mutex at parse time, so at most
/// one boolean is `true` here. Returns `None` for the bare invocation,
/// mapping to the engine's tombstone-driven default (residue with
/// tombstone → `Reattach`; residue without → refuse).
fn recovery_from_flags(
    reattach: bool,
    force_overwrite: bool,
    hard_cleanup_first: bool,
) -> Option<memstead_engine::RecoveryAction> {
    if reattach {
        Some(memstead_engine::RecoveryAction::Reattach)
    } else if force_overwrite {
        Some(memstead_engine::RecoveryAction::ForceOverwrite)
    } else if hard_cleanup_first {
        Some(memstead_engine::RecoveryAction::HardCleanupFirst)
    } else {
        None
    }
}

pub fn run_list(ctx: &CliContext, _args: ListArgs) -> anyhow::Result<()> {
    let setup_ctx = CliContext { json: ctx.json, quiet: ctx.quiet };
    let engine = crate::setup::pro_engine(&setup_ctx).map_err(|e| {
        generic_error(format!("mem list: could not initialize engine: {e}"))
    })?;

    let mut rows: Vec<serde_json::Value> = Vec::new();
    for name in engine.mem_names() {
        let cfg = engine
            .mem_configs_named()
            .find(|(n, _)| *n == name)
            .map(|(_, c)| c);
        let entity_count = engine
            .store()
            .all_entities()
            .filter(|e| e.id.mem() == name && !e.stub)
            .count();
        let capability = if engine.mem_router().is_writable(name) {
            "write"
        } else {
            "read_only"
        };
        rows.push(serde_json::json!({
            "name": name,
            "schema_ref": cfg.and_then(|c| c.schema.as_ref()).map(|s| s.to_string()),
            "version": cfg.and_then(|c| c.version.clone()),
            "entity_count": entity_count,
            "capability": capability,
        }));
    }

    if ctx.json {
        crate::output::print_json(&serde_json::json!({ "mems": rows }))?;
        return Ok(());
    }

    let mut lines: Vec<String> = vec![format!("# Mems ({})", rows.len()), String::new()];
    if rows.is_empty() {
        lines.push("_no mems mounted_".to_string());
    } else {
        for v in &rows {
            let name = v["name"].as_str().unwrap_or("?");
            let schema = v["schema_ref"].as_str().unwrap_or("—");
            let version = v["version"].as_str().unwrap_or("—");
            let count = v["entity_count"].as_u64().unwrap_or(0);
            let cap = v["capability"].as_str().unwrap_or("?");
            lines.push(format!(
                "- `{name}` ({cap}) — schema `{schema}`, version `{version}`, {count} entities"
            ));
        }
    }
    crate::output::print_markdown(&lines.join("\n"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memstead_base::EngineError;
    use memstead_base::ReferrerInfo;
    use memstead_engine::ProEngineError;
    use std::path::PathBuf;

    fn lifted_cli_error(err: ProEngineError) -> CliError {
        let any = pro_engine_err_to_cli(err);
        any.downcast::<CliError>()
            .expect("pro_engine_err_to_cli must lift to a CliError")
    }

    /// The client-side mem-template consumer surfaces a built-in
    /// schema's instance guidance keys when `--write-guidance` is
    /// omitted, stays silent when guidance is given, and is silent for a
    /// schema that ships no template.
    #[test]
    fn mem_template_guidance_note_surfaces_builtin_keys() {
        let planning: memstead_schema::SchemaRef = "planning@0.1.0".parse().unwrap();
        let note = mem_template_guidance_note(&planning, false)
            .expect("planning ships a mem-template — a note is due");
        assert!(note.contains("phase_context"), "note names the key: {note}");
        assert!(note.contains("--write-guidance"), "note tells how to fill: {note}");
        // Operator supplied guidance → nothing to surface.
        assert!(mem_template_guidance_note(&planning, false).is_some());
        assert!(mem_template_guidance_note(&planning, true).is_none());
        // A schema with no mem-template → no note.
        let default_: memstead_schema::SchemaRef = "default@1.0.0".parse().unwrap();
        assert!(mem_template_guidance_note(&default_, false).is_none());
    }

    /// The CLI's mem command surface does not translate the engine's
    /// typed code through a static table — a code added on the engine
    /// side reaches the CLI envelope unchanged. Pins the regression
    /// where `MEM_HAS_INCOMING_REFS` silently degraded to
    /// `VALIDATION_FAILED`.
    #[test]
    fn mem_has_incoming_refs_keeps_typed_code_and_carries_details() {
        let err = ProEngineError::Basis(EngineError::MemHasIncomingRefs {
            mem: "other".to_string(),
            referrers: vec![ReferrerInfo {
                from_id: "test--source".to_string(),
                rel_types: vec!["USES".to_string()],
                mem: "test".to_string(),
            }],
        });
        let cli = lifted_cli_error(err);
        assert_eq!(cli.code, "MEM_HAS_INCOMING_REFS");
        assert_eq!(cli.kind, ExitKind::Validation);
        let details = cli.details.expect("details must reach the CLI envelope");
        assert_eq!(details["mem"], "other");
        let referrers = details["referrers"].as_array().expect("referrers array");
        assert_eq!(referrers.len(), 1);
        assert_eq!(referrers[0]["from_id"], "test--source");
        assert_eq!(referrers[0]["mem"], "test");
    }

    /// Lifecycle refusal (a pro-only variant) is
    /// promoted through with the same code + structured details the
    /// MCP wire ships. `MEM_PATH_NOT_ALLOWED` carries the candidate,
    /// the patterns list, and the typed reason discriminator.
    #[test]
    fn mem_path_not_allowed_carries_structured_details() {
        let err = ProEngineError::MemPathNotAllowed {
            attempted: PathBuf::from("/ws/bogus"),
            candidate: "bogus".to_string(),
            patterns: vec!["specs".to_string(), "team/*".to_string()],
            reason: "no_match",
            policy_table: "mem_management.create",
        };
        let cli = lifted_cli_error(err);
        assert_eq!(cli.code, "MEM_PATH_NOT_ALLOWED");
        assert_eq!(cli.kind, ExitKind::Validation);
        let details = cli.details.expect("details");
        assert_eq!(details["candidate"], "bogus");
        assert_eq!(details["reason"], "no_match");
        assert_eq!(details["patterns"][0], "specs");
        // The `policy_table` disambiguator reaches the CLI envelope.
        assert_eq!(details["policy_table"], "mem_management.create");
        assert_eq!(details["patterns"][1], "team/*");
    }

    /// `VALIDATION_FAILED` is not
    /// used as the fallback for engine-sourced refusals. A
    /// typed lifecycle variant must not degrade to the catch-all.
    #[test]
    fn lifecycle_refusal_never_degrades_to_validation_failed_token() {
        let cases = [
            ProEngineError::MemPathNotAllowed {
                attempted: PathBuf::from("/x"),
                candidate: "x".to_string(),
                patterns: vec![],
                reason: "no_allowlist_configured",
                policy_table: "mem_management.create",
            },
            ProEngineError::MemReferencedByPolicy {
                name: "x".to_string(),
                referring_mems: vec!["y".to_string()],
            },
            ProEngineError::MemSchemaNotAllowed {
                candidate: "x".to_string(),
                matched_pattern: "p".to_string(),
                requested_schema: "default@1.0.0".to_string(),
                allowed_schemas: vec!["other@1.0.0".to_string()],
            },
            ProEngineError::InvalidMemName {
                name: "BadName".to_string(),
                reason: "invalid_char",
            },
        ];
        for err in cases {
            let cli = lifted_cli_error(err);
            assert_ne!(
                cli.code, "VALIDATION_FAILED",
                "engine-sourced refusal must carry its typed code: got {} with details {:?}",
                cli.code, cli.details,
            );
        }
    }

    /// Wrapped basis errors keep the
    /// per-variant exit-kind mapping (`NotFound` → exit 3,
    /// `HashMismatch` → exit 4, etc.) by delegating to
    /// `CliError::from_engine_op`. The lift doesn't flatten every
    /// basis variant to `Validation`.
    #[test]
    fn wrapped_basis_error_preserves_per_variant_exit_kind() {
        let err = ProEngineError::Basis(EngineError::NotFound {
            id: "specs--missing".to_string(),
        });
        let cli = lifted_cli_error(err);
        assert_eq!(cli.code, "ENTITY_NOT_FOUND");
        assert_eq!(cli.kind, ExitKind::NotFound);
        assert_eq!(cli.details.as_ref().unwrap()["id"], "specs--missing");
    }
}
