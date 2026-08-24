//! Top-level run-brief rendering — the one engine entry point every
//! consuming surface calls (the CLI via `memstead projection brief`), so the
//! brief a client emits is byte-identical to the CLI's **by construction**
//! (a single code path), not by parallel re-implementation.
//!
//! Given a loaded [`Engine`], the workspace root, and an ingest name, it
//! loads the four-primitive config, resolves the ingest, and — for discovery
//! mode — assembles the full brief: writing guidance from the destination
//! mem's schema + config, the paired-process-mem view, and the changed-slice
//! preface from live source state.

use std::path::Path;

use crate::Engine;
use crate::binding::{Binding, BuildMode};
use crate::pipeline_store::{BindingConfigs, load_pipeline_configs};

use super::brief::{
    ProcessMemInfo, assemble_discovery_brief, assemble_one_shot_brief, render_changed_slice,
    render_sync_brief, render_verify_brief,
};
use super::check_path::write_active_binding_file;
use super::cursor::compute_source_cursor;
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

/// If any primary source declares a preparation the engine's registry
/// ([`crate::preparation`]) does not know, return the unsupported-and-skipped
/// message; `None` when every declared preparation is registered (or none
/// is declared). Mirrors [`crate::binding::validate_binding`]'s registry
/// rule for a record that acquired an unknown identifier by hand — accepted
/// at rest, refused here rather than run over content the engine cannot
/// prepare — so the two refusal paths carry one semantics.
fn preparation_refusal(resolved: &ResolvedIngest) -> Option<String> {
    resolved.sources.iter().find_map(|s| match s {
        ResolvedSource::Primary(p) => p
            .preparation
            .as_deref()
            .filter(|prep| !crate::preparation::is_registered(prep))
            .map(|prep| {
                format!(
                    "> **[ingest] Ingest \"{}\" is unsupported: source \"{}\" declares \
                     preparation \"{}\", which is not in this engine's preparation registry \
                     (registered: {}). Skipping.**\n",
                    resolved.name,
                    p.name,
                    prep,
                    crate::preparation::registered_identifiers().join(", ")
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

/// Locate a binding by the CLI argument. The canonical form is the
/// binding id `<mem>/<stem>` (D3) — the shape `projection brief` / `--all`
/// selection use. As a transition bridge, a slash-free legacy argument (the
/// old flat ingest stem, e.g. `engine-graph`) is also matched against each
/// binding's `<mem>-<stem>` dashed form, so `memstead projection brief engine-graph`
/// keeps rendering the migrated `engine/graph` binding without a router change.
/// Returns the canonical binding id and the binding.
fn find_binding<'a>(
    configs: &'a BindingConfigs,
    arg: &str,
) -> Result<(String, &'a Binding), ResolveError> {
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
/// The single engine entry point behind every consuming surface. `ingest_name` is
/// the canonical binding id (or a legacy flat-ingest stem — see [`find_binding`]).
///
/// `consume` mirrors the scheduler's peek/consume split (decision 12,
/// backlog-sweep plan 03) onto derived caches: a peek (`false`) is a
/// pure read that leaves every cache byte-identical, while a consuming
/// render (`true`) additionally publishes this binding as the ACTIVE
/// one for deny enforcement (`projection check-path`). Without this, a
/// peek of binding A repointed enforcement so a later consuming run of
/// binding B was briefly guarded by A's denies.
pub fn render_ingest_brief(
    engine: &Engine,
    workspace_root: &Path,
    ingest_name: &str,
    consume: bool,
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

    let resolved = resolve_binding_run(&binding_id, binding)?;

    // Publish this binding as the ACTIVE one for the deny enforcement path
    // (`projection check-path` resolves "active" through this pointer) —
    // stale-safe (remove-then-write), overwrite-always, before any mode branch
    // so the channel is live for every consumed brief and never pins a
    // previous binding. Only the id is published; the deny list itself is
    // read fresh from the binding record on every check. Best-effort engine
    // cache, not a tracked mutation. Consuming renders only: a peek changes
    // no state a later actor depends on — derived caches included.
    if consume {
        write_active_binding_file(workspace_root, &binding_id);
    }

    // Refuse an ingest whose source declares a preparation the registry does
    // not know (a hand-edited record — every edit path refuses it earlier):
    // reported unsupported and skipped rather than run against content the
    // engine cannot prepare. A registered preparation passes.
    if let Some(message) = preparation_refusal(&resolved) {
        return Ok(message);
    }

    match resolved.mode {
        BuildMode::Discovery => Ok(render_discovery(engine, &resolved, workspace_root)),
        BuildMode::OneShot => Ok(render_one_shot(engine, &resolved)),
    }
}

/// Render the **verify brief** (C1) for a binding — the measurement +
/// capped-adjudication prompt an agent consumes. The one engine entry point
/// behind the CLI (`projection brief --verify`), mirroring
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
    let resolved = resolve_binding_run(&binding_id, binding)?;

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
/// point behind the CLI (`projection brief --sync`). It assembles both
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
    let resolved = resolve_binding_run(&binding_id, binding)?;

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

/// The `--medium-type` flag value for a medium — the wire spelling a
/// caller can paste back into `projection init`.
fn medium_type_wire(t: crate::pipeline::MediumType) -> &'static str {
    use crate::pipeline::MediumType as M;
    match t {
        M::Codebase => "codebase",
        M::Filesystem => "filesystem",
        M::Git => "git",
        M::Graph => "graph",
        M::Web => "web",
    }
}

/// Primary source names whose medium base does not exist on disk. Only
/// path-namespace media can be checked this way; a `web` or `graph`
/// pointer is out of scope and never reported absent.
fn absent_source_names(resolved: &ResolvedIngest, workspace_root: &Path) -> Vec<String> {
    resolved
        .sources
        .iter()
        .filter_map(|s| match s {
            ResolvedSource::Primary(p) => Some(p),
            ResolvedSource::Reference { .. } => None,
        })
        .filter(|p| {
            matches!(
                p.medium_type,
                crate::pipeline::MediumType::Codebase
                    | crate::pipeline::MediumType::Filesystem
                    | crate::pipeline::MediumType::Git
            ) && !super::cursor::medium_base(&p.pointer, workspace_root).exists()
        })
        .map(|p| p.name.clone())
        .collect()
}

/// A schema pin the reader can copy verbatim into `allow-create` and
/// `mem init`. Prefers one already in use in this workspace, so a mem
/// created by following a remedy speaks its neighbours' vocabulary; falls
/// back to the newest builtin `default` generation when the workspace has
/// no mem yet (the shape a fresh `mem-repo init` leaves behind). The
/// version is resolved from the registry rather than written literally, so
/// a schema generation bump cannot leave this remedy naming a stale pin.
fn suggested_schema_pin(engine: &Engine, writable: &[&str]) -> String {
    writable
        .iter()
        .find_map(|m| engine.schema_pin(m))
        .map(|r| r.as_display())
        .or_else(|| {
            memstead_schema::SchemaRegistry::builtin()
                .available_versions("default")
                .into_iter()
                .max()
                .map(|v| format!("default@{v}"))
        })
        // Unreachable with a sane binary: the builtin catalogue always
        // carries `default`. A placeholder is still better than a pin that
        // does not exist.
        .unwrap_or_else(|| "<name@version>".to_string())
}

/// The note the Destination block carries when the destination mem is not
/// in this workspace — and the remedy that actually works in the shape the
/// reader is standing in. `memstead mem init` is mem-repo-only, so naming
/// it unconditionally hands a filesystem-mem reader (the shape `memstead
/// quickstart` produces) a command that refuses; there, the binding is
/// simply pointed at the wrong mem and repointing it is the whole fix.
fn absent_destination_note(
    engine: &Engine,
    resolved: &ResolvedIngest,
    binding_id: &str,
    workspace_root: &Path,
) -> Option<String> {
    let dest = resolved.destination_mem.as_str();
    if engine.schema_pin(dest).is_some() {
        return None;
    }
    let mut writable: Vec<&str> = engine
        .mem_router()
        .writable_mems()
        .iter()
        .map(String::as_str)
        .collect();
    writable.sort_unstable();
    let remedy = if crate::workspace_store::is_mem_repo_shaped(workspace_root) {
        // `mem init` is refused by default: a mem-repo workspace creates
        // nothing until a `[[mem_management.create]]` rule admits the name.
        // Naming the second step only would hand the reader a command that
        // refuses `MEM_PATH_NOT_ALLOWED` on a workspace fresh from
        // `mem-repo init` — which is the workspace this brief most often
        // renders against.
        let admitted =
            crate::mem_management::CreateRuleSet::new(engine.settings().mem_create_rules.clone())
                .ok()
                .is_some_and(|set| set.matches(std::path::Path::new(dest)));
        // Both steps name the SAME concrete pin. A placeholder here would be
        // the one point on the first-session path where the reader must fetch
        // vocabulary from somewhere else; and naming a pin on the rule while
        // letting `mem init` fall back to its own default would refuse when
        // the two disagree.
        let pin = suggested_schema_pin(engine, &writable);
        if admitted {
            format!("Create it before writing: `memstead mem init {dest} --schema {pin}`.")
        } else {
            format!(
                "Creating it takes two steps — this workspace admits no mem name yet, \
                 so `memstead mem init` alone refuses: `memstead workspace allow-create \
                 '{dest}' --schema {pin}`, then `memstead mem init {dest} --schema {pin}`."
            )
        }
    } else if writable.is_empty() {
        "This workspace has no writable mem to point it at.".to_string()
    } else {
        // Re-declare rather than hand-edit. The record's LOCATION decides
        // the binding id and the mem whose anchors resolve — editing
        // `destination_mem` in place leaves the record under the wrong mem
        // folder, and every anchored write the brief mandates still refuses
        // with INVALID_ANCHOR. Naming the field alone would be a remedy the
        // reader could follow exactly and still be stuck.
        let stem = binding_id.rsplit('/').next().unwrap_or(binding_id);
        let redeclare = resolved
            .sources
            .iter()
            .find_map(|s| match s {
                crate::ingest::resolve::ResolvedSource::Primary(p) => Some(p),
                crate::ingest::resolve::ResolvedSource::Reference { .. } => None,
            })
            .map(|p| {
                format!(
                    " Re-declare it against that mem: `rm .memstead/projections/{binding_id}.json` \
                     then `memstead projection init --mem {} --source {} --medium-type {} \
                     --name {}`.",
                    writable.first().copied().unwrap_or("<mem>"),
                    p.pointer,
                    medium_type_wire(p.medium_type),
                    // `--name` is not optional here: a `.` pointer (the
                    // `quickstart --repo .` layout) derives no stem and
                    // refuses PROJECTION_INVALID_NAME without it.
                    stem,
                )
            })
            .unwrap_or_default();
        format!(
            "This is a filesystem-mem workspace, which holds one mem and cannot \
             add another, so this binding names a mem that can never exist here.{redeclare} \
             Editing `destination_mem` alone is not enough — the record's folder \
             decides which mem's anchors resolve."
        )
    };
    Some(format!(
        "**This mem does not exist in this workspace yet.** {remedy} Until then, \
         every mutation this brief asks for will refuse."
    ))
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

    let dest_note = absent_destination_note(engine, resolved, &resolved.name, workspace_root);
    let absent = absent_source_names(resolved, workspace_root);
    assemble_discovery_brief(
        resolved,
        &guidance,
        &process_mem,
        dest_schema.as_deref(),
        dest_note.as_deref(),
        &absent,
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
        // The one-shot lens has no workspace root in hand; its destination
        // set is validated by the lens block itself.
        None,
        &[],
        dest_purpose.as_deref(),
    )
}

/// Resolve the paired-process-mem view from live workspace state. Read-only:
/// a missing process mem is reported absent rather than auto-created (mutation
/// belongs to the orchestration layer, not brief rendering).
fn build_process_mem(engine: &Engine, resolved: &ResolvedIngest) -> ProcessMemInfo {
    let skipped = resolved.mode == BuildMode::OneShot;
    // One resolution mechanism (agent-trust plan 14): the
    // destination's declaration wins, the ingest-name convention is
    // the fallback. A declared-but-unmounted process mem is a stated
    // notice, never a silent fallback to derivation.
    let resolution = crate::ingest::resolve::resolve_process_mem(
        engine,
        &resolved.destination_mem,
        &resolved.name,
    );
    let leaf = resolution.mem.clone();
    let present = !skipped && resolution.mounted;
    let notice = (!skipped && resolution.declared && !resolution.mounted).then(|| {
        format!(
            "destination `{}` declares process mem `{}`, which is not mounted",
            resolved.destination_mem, resolution.mem
        )
    });
    ProcessMemInfo {
        present,
        skipped,
        notice,
        mem_label: if resolution.declared {
            leaf.clone()
        } else {
            format!("ingest/{leaf}")
        },
        leaf_name: leaf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::BuildMode;
    use crate::ingest::resolve::Source;
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
        ResolvedSource::Primary(Source {
            name: facet.to_string(),
            medium_type: MediumType::Codebase,
            pointer: String::new(),
            change_detection: None,
            scope: vec![],
            engagement: None,
            preparation: preparation.map(str::to_string),
        })
    }

    /// An ingest whose source declares a preparation the registry does not
    /// know is refused (unsupported / skip) rather than rendered — the same
    /// rule `validate_binding` applies, mirrored for a hand-edited record.
    /// A registered preparation passes, and the message speaks of sources,
    /// never of the retired facet noun.
    #[test]
    fn unregistered_preparation_is_refused_registered_passes() {
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
        assert_eq!(
            preparation_refusal(&ingest_with(vec![primary(
                "claims",
                Some(crate::preparation::ENTITY_LOAD_BEARING),
            )])),
            None,
            "a registered preparation is not refused at render"
        );
        let msg = preparation_refusal(&ingest_with(vec![primary(
            "manuals",
            Some("pdf-to-markdown"),
        )]))
        .unwrap();
        assert_eq!(
            msg,
            format!(
                "> **[ingest] Ingest \"ing\" is unsupported: source \"manuals\" declares preparation \"pdf-to-markdown\", which is not in this engine's preparation registry (registered: {}). Skipping.**\n",
                crate::preparation::registered_identifiers().join(", ")
            )
        );
        assert!(msg.contains("entity-load-bearing, dated-entries, code-map"));
        assert!(!msg.contains("facet"));
    }
}
