//! `memstead changes` — diff a mem's HEAD against a caller-provided SHA.
//!
//! Mirrors the MCP `memstead_changes_since` tool so the same commit-SHA
//! response field can drive both interactive (CLI) and agent (MCP)
//! polling flows.

use clap::Parser;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

#[derive(Parser, Debug)]
pub struct Args {
    /// Writable mem name. Defaults to the first loaded mem.
    #[arg(long)]
    pub mem: Option<String>,

    /// Commit SHA to diff against. Pass a prior mutation's `commit_sha`,
    /// or the git canonical empty-tree hash
    /// `4b825dc642cb6eb9a060e54bf8d69288fbee4904` for a fresh-client
    /// first sync.
    #[arg(long)]
    pub since: String,

    /// Rename detection threshold in [0.1, 1.0]; mirrors the MCP
    /// `rename_similarity` parameter. Default 0.6. Engine-authored
    /// renames pair via commit-note provenance and bypass this
    /// threshold; the value drives the rename-similarity fallback for
    /// non-engine renames (external `git mv`, pre-provenance
    /// migrations). Lower widens the recall window at the cost of
    /// false-positive pairing on that path.
    #[arg(long)]
    pub rename_similarity: Option<f32>,

    /// Fold per-commit agent-notes (subject, note, actor, tool, client)
    /// and the workspace-level schema/registry ref tip (unified schemas +
    /// per-mem configs) into the response. Default off — entity-
    /// delta only. Outer-repo auto-commit consumers turn this on so
    /// they get notes + the registry-ref sha in one round-trip without
    /// re-walking the gitdir.
    #[arg(long)]
    pub include_notes: bool,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => run_mem_repo(ctx, engine, args),
        CliEngine::Filesystem(engine) => run_filesystem(ctx, engine, args),
    }
}

#[cfg(feature = "mem-repo")]
fn run_mem_repo(ctx: &CliContext, engine: memstead_base::Engine, args: Args) -> anyhow::Result<()> {
    let mem = match args.mem {
        Some(v) => v,
        None => engine
            .mem_configs_named()
            .find(|(name, _)| engine.mem_router().is_writable(name))
            .map(|(name, _)| name.to_string())
            .ok_or_else(|| {
                CliError::new(
                    ExitKind::Generic,
                    "NO_WRITABLE_MEM",
                    "no writable mem loaded — pass --mem <name>",
                )
            })?,
    };

    let mut report = engine
        .changes_since(&mem, &args.since, args.rename_similarity)
        .map_err(CliError::from_engine_op)?;

    // The engine unconditionally populates `notes` and `memstead_ref`
    // on every git-branch backend call (the engine layer has the
    // gitdir access, the renderer layer knows the caller's intent).
    // The CLI is the renderer that filters:
    // strip both fields when the flag is absent so a caller switching
    // between `--include-notes` and the default sees the documented
    // two-shape response.
    if !args.include_notes {
        report.notes = None;
        report.memstead_ref = None;
    }

    if ctx.json {
        print_json(&report)?;
        return Ok(());
    }

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "# Changes in `{}` since `{}`",
        report.mem, report.since
    ));
    lines.push(String::new());
    lines.push(format!("- HEAD: `{}`", report.head));
    lines.push(format!("- Changes: {}", report.changes.len()));
    lines.push(String::new());

    if report.changes.is_empty() {
        lines.push("_no changes_".to_string());
    } else {
        for change in &report.changes {
            use memstead_git_branch::ChangeEnvelope::*;
            let type_suffix =
                |t: &Option<String>| t.as_ref().map(|s| format!(" [{s}]")).unwrap_or_default();
            let title_suffix =
                |t: &Option<String>| t.as_ref().map(|s| format!(" — {s}")).unwrap_or_default();
            let line = match change {
                Added {
                    id,
                    title,
                    entity_type,
                } => format!(
                    "- **added** `{}`{}{}",
                    id,
                    type_suffix(entity_type),
                    title_suffix(title)
                ),
                Updated {
                    id,
                    title,
                    entity_type,
                } => format!(
                    "- **updated** `{}`{}{}",
                    id,
                    type_suffix(entity_type),
                    title_suffix(title)
                ),
                Removed {
                    id,
                    title,
                    entity_type,
                } => format!(
                    "- **removed** `{}`{}{}",
                    id,
                    type_suffix(entity_type),
                    title_suffix(title)
                ),
                Renamed {
                    from_id,
                    to_id,
                    title,
                    entity_type,
                } => format!(
                    "- **renamed** `{from_id}` → `{to_id}`{}{}",
                    type_suffix(entity_type),
                    title_suffix(title)
                ),
            };
            lines.push(line);
        }
    }

    if let Some(notes) = report.notes.as_ref() {
        lines.push(String::new());
        lines.push(format!("## Agent notes ({})", notes.len()));
        if notes.is_empty() {
            lines.push("_no commits in range_".to_string());
        } else {
            for n in notes {
                let actor = n.actor.as_deref().unwrap_or("unknown");
                let subject = if n.subject.is_empty() {
                    "(no subject)"
                } else {
                    n.subject.as_str()
                };
                lines.push(format!(
                    "- `{}` [{}] {}",
                    &n.sha[..n.sha.len().min(12)],
                    actor,
                    subject
                ));
                // Multi-entity commits (batch-update) collapse their
                // subject to `(N entities)`; name the entities so the note
                // is self-describing here, not only in the JSON envelope.
                if !n.entity_ids.is_empty() {
                    lines.push(format!("    entities: {}", n.entity_ids.join(", ")));
                }
                if let Some(note) = n.note.as_deref() {
                    for body_line in note.lines() {
                        lines.push(format!("    {body_line}"));
                    }
                }
            }
        }
    }

    if let Some(sha) = report.memstead_ref.as_deref() {
        lines.push(String::new());
        lines.push("## Registry ref".to_string());
        lines.push(format!("- `__MEMSTEAD`: `{sha}`"));
    }

    print_markdown(&lines.join("\n"));
    Ok(())
}

/// Filesystem-mem `memstead changes` reads `.memstead/changes.jsonl` and
/// returns entries with `ts > since`. The cursor is a timestamp
/// string (RFC 3339), not a commit-SHA — same divergence as the
/// MCP `memstead_changes_since` tool's filesystem path. `--rename-similarity`
/// and `--include-notes` are accepted for shape parity but ignored:
/// filesystem-mem has no rename detection (mutations are explicit
/// in the changelog) and no agent-notes layer (notes ride on each
/// changelog entry).
fn run_filesystem(
    ctx: &CliContext,
    engine: memstead_base::Engine,
    args: Args,
) -> anyhow::Result<()> {
    let workspace_mem = engine
        .mem_names()
        .into_iter()
        .next()
        .map(String::from)
        .unwrap_or_default();
    if let Some(name) = args.mem.as_deref()
        && name != workspace_mem
    {
        return Err(CliError::new(
                ExitKind::NotFound,
                "UNKNOWN_MEM",
                format!(
                    "filesystem-mem is single-mem: workspace mem is `{workspace_mem}`, --mem `{name}` does not match"
                ),
            )
            .into());
    }

    // Unified engine doesn't expose workspace_root (mounts can be
    // heterogeneous); discover from cwd.
    let workspace_root =
        crate::setup::find_filesystem_workspace_root(&std::env::current_dir().map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                crate::INTERNAL_CODE,
                format!("current_dir: {e}"),
            )
        })?)
        .ok_or_else(|| {
            CliError::new(
                ExitKind::NotFound,
                "WORKSPACE_NOT_INITIALISED",
                "no filesystem-mem workspace found from cwd",
            )
        })?;
    let log_path = workspace_root
        .join(memstead_base::MEM_META_DIR)
        .join("changes.jsonl");
    let raw = match std::fs::read_to_string(&log_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(CliError::new(
                ExitKind::Generic,
                crate::INTERNAL_CODE,
                format!("read {}: {e}", log_path.display()),
            )
            .into());
        }
    };

    let since = args.since.trim();
    let mut entries: Vec<serde_json::Value> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue, // skip malformed lines silently
        };
        let ts_match = value
            .get("ts")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !since.is_empty() && ts_match.as_str() <= since {
            continue;
        }
        entries.push(value);
    }

    if ctx.json {
        print_json(&serde_json::json!({
            "mem": workspace_mem,
            "since": since,
            "entries": entries,
        }))?;
        return Ok(());
    }

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "# Changes in `{}` since `{}`",
        workspace_mem, since
    ));
    lines.push(String::new());
    lines.push(format!("- Entries: {}", entries.len()));
    lines.push(String::new());
    if entries.is_empty() {
        lines.push("_no changes_".to_string());
    } else {
        for entry in &entries {
            let kind = entry.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
            let id = entry
                .get("entity")
                .and_then(|v| v.as_str())
                .unwrap_or("(no entity)");
            let ts = entry.get("ts").and_then(|v| v.as_str()).unwrap_or("?");
            let note = entry
                .get("note")
                .and_then(|v| v.as_str())
                .map(|s| format!(" — {s}"))
                .unwrap_or_default();
            lines.push(format!("- `{ts}` **{kind}** `{id}`{note}"));
        }
    }
    print_markdown(&lines.join("\n"));
    Ok(())
}
