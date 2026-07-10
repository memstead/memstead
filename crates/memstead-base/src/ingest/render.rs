//! Top-level run-brief rendering — the one engine entry point that both the
//! CLI (`memstead ingest brief`) and UniFFI (macOS app) call, so the brief a
//! client emits is byte-identical to the CLI's **by construction** (a single
//! code path), not by parallel re-implementation.
//!
//! Given a loaded [`Engine`], the workspace root, and an ingest name, it
//! loads the four-primitive config, resolves the ingest, and — for discovery
//! mode — assembles the full brief: writing guidance from the destination
//! mem's schema + config, the paired-process-mem view, and the changed-slice
//! preface from live source state.

use std::path::Path;

use crate::Engine;
use crate::pipeline::IngestMode;
use crate::pipeline_store::load_pipeline_configs;

use super::brief::{
    ProcessMemInfo, assemble_discovery_brief, assemble_one_shot_brief, assemble_refinement_brief,
    render_changed_slice,
};
use super::cursor::{compute_source_cursor, write_active_deny_file};
use super::guidance::{GuidanceDefaults, MemGuidance, ResolvedGuidance, resolve_writing_guidance};
use super::refinement::{
    clear_findings, next_batch, read_pending_findings, render_refinement_scout,
    render_refinement_writer,
};
use super::resolve::{ResolveError, ResolvedIngest, ResolvedSource, resolve_ingest};

/// Why [`render_ingest_brief`] could not produce a brief.
#[derive(Debug, thiserror::Error)]
pub enum RenderBriefError {
    /// The four-primitive pipeline config could not be loaded.
    #[error("could not load pipeline config: {0}")]
    ConfigLoad(String),
    /// The ingest (or a reference it names) could not be resolved.
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    /// The ingest is not discovery mode; refinement / one-shot briefs are not
    /// yet rendered by the engine.
    #[error("ingest '{name}' is {mode} mode; only discovery-mode briefs are rendered so far")]
    ModeUnsupported {
        /// The ingest name.
        name: String,
        /// The unsupported mode (`refinement` / `one-shot`).
        mode: String,
    },
}

/// If any source facet declares an (unimplemented) preparation step, return the
/// unsupported-and-skipped message the plugin's preparation guard emits; `None`
/// when every source is directly ingestable. No preparation implementation
/// exists, so *any* declared preparation is unsupported.
fn preparation_refusal(resolved: &ResolvedIngest) -> Option<String> {
    resolved.sources.iter().find_map(|s| match s {
        ResolvedSource::Primary(p) => p.preparation.as_deref().map(|prep| {
            format!(
                "> **[ingest] Ingest \"{}\" is unsupported: facet \"{}\" declares preparation \
                 \"{}\", which has no implementation. Skipping.**\n",
                resolved.name, p.facet_ref, prep
            )
        }),
        ResolvedSource::Reference { .. } => None,
    })
}

/// The mode string used in messages (`discovery` / `refinement` / `one-shot`).
pub fn mode_name(mode: IngestMode) -> &'static str {
    match mode {
        IngestMode::Discovery => "discovery",
        IngestMode::Refinement => "refinement",
        IngestMode::OneShot => "one-shot",
    }
}

/// Render the run-brief for an ingest — the Markdown prompt an agent
/// consumes. The single engine entry point shared by the CLI and UniFFI.
pub fn render_ingest_brief(
    engine: &Engine,
    workspace_root: &Path,
    ingest_name: &str,
) -> Result<String, RenderBriefError> {
    let configs = load_pipeline_configs(workspace_root)
        .map_err(|e| RenderBriefError::ConfigLoad(e.to_string()))?;
    let resolved = resolve_ingest(&configs, ingest_name)?;

    // Publish this ingest's deny list for the plugin's PreToolUse deny hook —
    // stale-safe (remove-then-write), overwrite-always, before any mode branch
    // so the channel is live for every rendered brief and never pins a
    // previous ingest's list. Best-effort engine cache, not a tracked mutation.
    write_active_deny_file(workspace_root, &resolved.name, &resolved.deny_paths);

    // Refuse an ingest whose source facet declares a deterministic preparation
    // step (e.g. `pdf-to-markdown`) — no preparation implementation exists, so
    // the ingest is reported unsupported and skipped rather than run against
    // raw, unprepared content. Mirrors the plugin's preparation guard.
    if let Some(message) = preparation_refusal(&resolved) {
        return Ok(message);
    }

    match resolved.mode {
        IngestMode::Discovery => Ok(render_discovery(engine, &resolved, workspace_root)),
        IngestMode::OneShot => Ok(render_one_shot(engine, &resolved)),
        IngestMode::Refinement => Ok(render_refinement(engine, &resolved, workspace_root)),
    }
}

/// Resolve the destination mem's writing guidance (schema defaults + per-mem
/// additions / legacy) — shared by the discovery and one-shot briefs.
fn dest_guidance(engine: &Engine, dest: &str) -> ResolvedGuidance {
    let defaults = engine
        .schema_for(dest)
        .and_then(|schema| schema.manifest.default_writing_guidance.clone())
        .map(|d| GuidanceDefaults {
            goal: d.goal,
            avoid: d.avoid,
        })
        .unwrap_or_default();

    let mem_guidance = engine
        .mem_config_for(dest)
        .map(|config| {
            let get = |key: &str| {
                config
                    .write_guidance
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            };
            MemGuidance {
                goal_additions: get("goal_additions"),
                avoid_additions: get("avoid_additions"),
                legacy_goal: get("goal"),
                legacy_avoid: get("avoid"),
            }
        })
        .unwrap_or_default();

    resolve_writing_guidance(&defaults, &mem_guidance)
}

/// Assemble the discovery brief from the engine's live view of the
/// destination mem: its schema defaults, per-mem writing-guidance additions,
/// pinned schema ref, paired-process-mem existence, and the source cursor.
fn render_discovery(engine: &Engine, resolved: &ResolvedIngest, workspace_root: &Path) -> String {
    let dest = &resolved.destination_mem;
    let guidance = dest_guidance(engine, dest);
    let dest_schema = engine.schema_pin(dest).map(|r| r.as_display());
    let process_mem = build_process_mem(engine, resolved);

    // Changed-slice preface from live source state (empty when nothing has
    // moved → the brief is byte-identical to a plain roam).
    let cursor = compute_source_cursor(engine, resolved, workspace_root);
    let preface = render_changed_slice(&cursor);

    assemble_discovery_brief(
        resolved,
        &guidance,
        &process_mem,
        dest_schema.as_deref(),
        &preface,
    )
}

/// Assemble the one-shot lens brief — no changed-slice, no paired process mem;
/// the destination-set / routing / idempotency / report lens block instead.
fn render_one_shot(engine: &Engine, resolved: &ResolvedIngest) -> String {
    let dest = &resolved.destination_mem;
    let guidance = dest_guidance(engine, dest);
    let dest_schema = engine.schema_pin(dest).map(|r| r.as_display());
    let dest_purpose = engine
        .mem_config_for(dest)
        .and_then(|c| c.description.clone());
    let process_mem = build_process_mem(engine, resolved); // skipped = true for one-shot

    assemble_one_shot_brief(
        resolved,
        &guidance,
        &process_mem,
        dest_schema.as_deref(),
        dest_purpose.as_deref(),
    )
}

/// Assemble the refinement brief — the discovery-style header plus the
/// scout-or-writer phase block. A pending findings file → writer pass (which
/// consumes it); otherwise the next scout batch.
fn render_refinement(engine: &Engine, resolved: &ResolvedIngest, workspace_root: &Path) -> String {
    let dest = &resolved.destination_mem;
    let guidance = dest_guidance(engine, dest);
    let dest_schema = engine.schema_pin(dest).map(|r| r.as_display());
    let process_mem = build_process_mem(engine, resolved);
    let preface = render_changed_slice(&compute_source_cursor(engine, resolved, workspace_root));

    let cache_root = workspace_root.join(".memstead.cache").join("ingest");
    let phase = if let Some(findings) = read_pending_findings(&cache_root, &resolved.name) {
        clear_findings(&cache_root, &resolved.name);
        render_refinement_writer(resolved, &findings)
    } else {
        next_batch(resolved, workspace_root, &cache_root)
            .map(|batch| render_refinement_scout(resolved, &batch, &cache_root))
            .unwrap_or_default()
    };

    assemble_refinement_brief(
        resolved,
        &guidance,
        &process_mem,
        dest_schema.as_deref(),
        &preface,
        &phase,
    )
}

/// Resolve the paired-process-mem view from live workspace state. Read-only:
/// a missing process mem is reported absent rather than auto-created (mutation
/// belongs to the orchestration layer, not brief rendering).
fn build_process_mem(engine: &Engine, resolved: &ResolvedIngest) -> ProcessMemInfo {
    let leaf = resolved.name.clone();
    let skipped = resolved.mode == IngestMode::OneShot;
    let present = !skipped && engine.mem_names().iter().any(|m| *m == leaf);
    ProcessMemInfo {
        present,
        skipped,
        notice: None,
        mem_label: format!("ingest/{leaf}"),
        leaf_name: leaf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::resolve::ResolvedPrimarySource;
    use crate::pipeline::{IngestTrigger, MediumType};

    fn ingest_with(sources: Vec<ResolvedSource>) -> ResolvedIngest {
        ResolvedIngest {
            name: "ing".to_string(),
            mode: IngestMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 20,
            deny_paths: vec![],
            projection_ref: "m/p".to_string(),
            projection_mem: "m".to_string(),
            projection_name: "p".to_string(),
            intent: None,
            sources,
            destination_mem: "m".to_string(),
            rules: None,
            post_actions: None,
        }
    }

    fn primary(facet: &str, preparation: Option<&str>) -> ResolvedSource {
        ResolvedSource::Primary(ResolvedPrimarySource {
            facet_ref: facet.to_string(),
            medium: "m".to_string(),
            medium_type: MediumType::Codebase,
            medium_pointer: String::new(),
            declared_change_detection: None,
            scope: vec![],
            preparation: preparation.map(str::to_string),
        })
    }

    /// An ingest whose source facet declares an unimplemented preparation step
    /// is refused (unsupported / skip) rather than rendered — the plugin's
    /// preparation guard, ported.
    #[test]
    fn preparation_step_is_refused() {
        assert_eq!(
            preparation_refusal(&ingest_with(vec![primary("f", None)])),
            None
        );
        assert_eq!(
            preparation_refusal(&ingest_with(vec![ResolvedSource::Reference {
                mem: "e".to_string()
            }])),
            None
        );
        let msg = preparation_refusal(&ingest_with(vec![primary(
            "manuals",
            Some("pdf-to-markdown"),
        )]))
        .unwrap();
        assert_eq!(
            msg,
            "> **[ingest] Ingest \"ing\" is unsupported: facet \"manuals\" declares preparation \"pdf-to-markdown\", which has no implementation. Skipping.**\n"
        );
    }
}
