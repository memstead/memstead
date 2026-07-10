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
use crate::binding::{BindingV1, BuildMode};
use crate::pipeline_store::{BindingConfigs, load_pipeline_configs};

use super::brief::{
    ProcessMemInfo, assemble_discovery_brief, assemble_one_shot_brief, render_changed_slice,
    render_sync_brief, render_verify_brief,
};
use super::cursor::{compute_source_cursor, write_active_deny_file};
use super::findings::{FindingClass, current_findings};
use super::guidance::{GuidanceDefaults, MemGuidance, ResolvedGuidance, resolve_writing_guidance};
use super::prune::prune_proposals;
use super::resolve::{ResolveError, ResolvedIngest, ResolvedSource, resolve_binding_run};

/// Why [`render_ingest_brief`] could not produce a brief.
#[derive(Debug, thiserror::Error)]
pub enum RenderBriefError {
    /// The four-primitive pipeline config could not be loaded.
    #[error("could not load pipeline config: {0}")]
    ConfigLoad(String),
    /// The ingest (or a reference it names) could not be resolved.
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    /// The binding declares no `build` operation, so the build path (brief) is
    /// refused (D6/AC4). The message carries the one-command remedy
    /// `memstead projection enable build <binding>`, which — run verbatim —
    /// makes the same brief succeed.
    #[error(
        "binding '{binding}' has no build operation — enable it with \
         `memstead projection enable build {binding}`"
    )]
    BuildOperationAbsent {
        /// The binding id whose build block is absent.
        binding: String,
    },
    /// The durable findings store could not be read while rendering a verify /
    /// sync brief (group C). The brief needs the open findings; a malformed
    /// store surfaces here rather than silently rendering an empty findings set.
    #[error("could not read findings store for '{binding}': {detail}")]
    FindingsRead {
        /// The binding id whose findings store failed to read.
        binding: String,
        /// The underlying store error, stringified.
        detail: String,
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

/// The mode string used in messages (`discovery` / `one-shot`).
pub fn mode_name(mode: BuildMode) -> &'static str {
    match mode {
        BuildMode::Discovery => "discovery",
        BuildMode::OneShot => "one-shot",
    }
}

/// Locate a binding by the CLI/UniFFI argument. The canonical form is the
/// binding id `<mem>/<stem>` (D3) — the shape `projection brief` / `--all`
/// selection use. As a transition bridge, a slash-free legacy argument (the
/// old flat ingest stem, e.g. `engine-graph`) is also matched against each
/// binding's `<mem>-<stem>` dashed form, so `memstead ingest brief engine-graph`
/// keeps rendering the migrated `engine/graph` binding without a router change.
/// Returns the canonical binding id and the binding.
fn find_binding<'a>(
    configs: &'a BindingConfigs,
    arg: &str,
) -> Result<(String, &'a BindingV1), ResolveError> {
    // Exact canonical id: `<mem>/<stem>`.
    if let Some(r) = configs
        .bindings
        .iter()
        .find(|r| format!("{}/{}", r.mem, r.name) == arg)
    {
        return Ok((format!("{}/{}", r.mem, r.name), &r.config));
    }
    // Transition bridge: a slash-free legacy stem → `<mem>-<stem>` dashed form.
    if !arg.contains('/')
        && let Some(r) = configs
            .bindings
            .iter()
            .find(|r| format!("{}-{}", r.mem, r.name) == arg)
    {
        return Ok((format!("{}/{}", r.mem, r.name), &r.config));
    }
    Err(ResolveError::BindingNotFound {
        name: arg.to_string(),
        available: configs
            .bindings
            .iter()
            .map(|r| format!("{}/{}", r.mem, r.name))
            .collect(),
    })
}

/// Render the run-brief for a binding — the Markdown prompt an agent consumes.
/// The single engine entry point shared by the CLI and UniFFI. `ingest_name` is
/// the canonical binding id (or a legacy flat-ingest stem — see [`find_binding`]).
pub fn render_ingest_brief(
    engine: &Engine,
    workspace_root: &Path,
    ingest_name: &str,
) -> Result<String, RenderBriefError> {
    let configs = load_pipeline_configs(workspace_root)
        .map_err(|e| RenderBriefError::ConfigLoad(e.to_string()))?;
    let (binding_id, binding) = find_binding(&configs, ingest_name)?;

    // D6/AC4: the build path (brief) refuses when the binding declares no build
    // operation, carrying the one-command `projection enable build` remedy —
    // rather than fabricating a default build the operator never declared.
    if binding.operations.build.is_none() {
        return Err(RenderBriefError::BuildOperationAbsent {
            binding: binding_id,
        });
    }

    let resolved = resolve_binding_run(&configs, &binding_id, binding)?;

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
        BuildMode::Discovery => Ok(render_discovery(engine, &resolved, workspace_root)),
        BuildMode::OneShot => Ok(render_one_shot(engine, &resolved)),
    }
}

/// Render the **verify brief** (C1) for a binding — the measurement +
/// capped-adjudication prompt an agent consumes. The one engine entry point the
/// CLI (`projection brief --verify`) and UniFFI share, mirroring
/// [`render_ingest_brief`]. Read-only on the destination mem: it borrows
/// `&Engine` (shared), reads the durable findings store for the backlog count,
/// and renders. It emits **no** destination-mutation instruction (C1) — the
/// refusal is carried by [`render_verify_brief`] itself.
pub fn render_verify_brief_for(
    engine: &Engine,
    workspace_root: &Path,
    binding_id: &str,
) -> Result<String, RenderBriefError> {
    let configs = load_pipeline_configs(workspace_root)
        .map_err(|e| RenderBriefError::ConfigLoad(e.to_string()))?;
    let (binding_id, binding) = find_binding(&configs, binding_id)?;
    let resolved = resolve_binding_run(&configs, &binding_id, binding)?;

    let (_key, findings) =
        current_findings(engine, workspace_root, binding, &resolved).map_err(|e| {
            RenderBriefError::FindingsRead {
                binding: binding_id.clone(),
                detail: e.to_string(),
            }
        })?;
    let backlog = findings
        .iter()
        .filter(|f| f.class == FindingClass::QueuedForAdjudication)
        .count();
    Ok(render_verify_brief(&resolved, backlog))
}

/// Render the **sync brief** (C2/C3) for a binding — the *single* channel
/// through which maintenance-writing work reaches an agent. The one engine entry
/// point the CLI (`projection brief --sync`) and UniFFI share. It assembles both
/// inputs in one render: the live cursor slice ([`compute_source_cursor`]) and
/// the open findings the verify pass recorded (`current(key)`), plus the adopt
/// framing when the mem predates its binding (E1). Read-only on the destination
/// mem (shared `&Engine`) — every repair happens only when an agent acts on this
/// brief through the normal MCP mutation surface.
pub fn render_sync_brief_for(
    engine: &Engine,
    workspace_root: &Path,
    binding_id: &str,
) -> Result<String, RenderBriefError> {
    let configs = load_pipeline_configs(workspace_root)
        .map_err(|e| RenderBriefError::ConfigLoad(e.to_string()))?;
    let (binding_id, binding) = find_binding(&configs, binding_id)?;
    let resolved = resolve_binding_run(&configs, &binding_id, binding)?;

    let cursor = compute_source_cursor(engine, &resolved, workspace_root);
    let (_key, findings) =
        current_findings(engine, workspace_root, binding, &resolved).map_err(|e| {
            RenderBriefError::FindingsRead {
                binding: binding_id.clone(),
                detail: e.to_string(),
            }
        })?;
    // Prune proposals (group F) ride the sync brief — the sole channel through
    // which a prune removal reaches the mem (F3/A5). Read-only gather.
    let prune = prune_proposals(engine, workspace_root, binding, &resolved);
    let adopt = mem_predates_binding(engine, &resolved);
    Ok(render_sync_brief(
        &resolved, &cursor, &findings, &prune, adopt,
    ))
}

/// Whether the destination mem predates its binding — the adopt / onboarding
/// signal (E1). True when the mem carries **no** anchors and the binding has
/// **no** recorded `#synced` baseline for any facet: there is nothing to diff
/// against and nothing anchored yet, so 0% anchored is expected (a first sync),
/// not drift. A genuinely-fresh mem legitimately gets the same first-sync
/// framing — the signal is deliberately generic.
///
/// The single canonical adopt predicate: the sync brief ([`render_sync_brief_for`]),
/// the tier-1 fidelity report ([`super::report::compute_fidelity_report`]), and the
/// status rollup ([`super::status::projection_rollup`]) all read it, so onboarding
/// framing and the no-red-verdict-from-pre-binding-history refusal stay in lockstep
/// across every surface.
pub fn mem_predates_binding(engine: &Engine, resolved: &ResolvedIngest) -> bool {
    let no_anchors = engine
        .mem_anchors_resolved(&resolved.destination_mem)
        .is_empty();
    let prefix = format!("{}/", resolved.name);
    let never_synced = engine
        .mem_config_for(&resolved.destination_mem)
        .map(|c| {
            !c.sync_state
                .keys()
                .any(|k| k.starts_with(&prefix) && k.ends_with("#synced"))
        })
        .unwrap_or(true);
    no_anchors && never_synced
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

/// Resolve the paired-process-mem view from live workspace state. Read-only:
/// a missing process mem is reported absent rather than auto-created (mutation
/// belongs to the orchestration layer, not brief rendering).
fn build_process_mem(engine: &Engine, resolved: &ResolvedIngest) -> ProcessMemInfo {
    let leaf = resolved.name.clone();
    let skipped = resolved.mode == BuildMode::OneShot;
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
    use crate::binding::BuildMode;
    use crate::ingest::resolve::ResolvedPrimarySource;
    use crate::pipeline::{IngestTrigger, MediumType};

    fn ingest_with(sources: Vec<ResolvedSource>) -> ResolvedIngest {
        ResolvedIngest {
            name: "ing".to_string(),
            mode: BuildMode::Discovery,
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
