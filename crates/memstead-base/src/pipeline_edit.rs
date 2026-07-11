//! Edit operations over the four-primitive pipeline store, with referential
//! integrity.
//!
//! Sits above the dumb file ops in [`crate::pipeline_store`]: each function
//! loads the current store, enforces the Medium ← Facet ← Projection ← Ingest
//! reference model (no clobber, no dangling reference), then writes. The
//! engine's in-memory `pipeline_configs` cache is refreshed by the `Engine`
//! wrapper methods that call these — not here — so these functions stay pure
//! disk ops that unit-test with a bare `TempDir`.
//!
//! Identity is the file stem `(mem, name)`. Mediums and facets additionally
//! carry an embedded `name` field kept equal to the stem (facets reference
//! mediums by name, projections reference facets by name); rename here updates
//! the file location, the embedded field, and every dependent reference
//! together, so the store never holds a self-inconsistent or dangling record.
//!
//! Scope: medium / facet / projection — the editing chain the macOS app's
//! pipeline editor drives. Ingests are read for integrity (a projection can't
//! be deleted while an ingest runs it) and repointed on a projection rename,
//! but interactive ingest editing has no consumer yet and is not exposed here.
//!
//! Projection edits operate on the versioned **binding record** (v1): the
//! JSON entry points ([`add_binding_json`] / [`update_binding_json`]) accept
//! a [`BindingPatch`] over the full author-editable record — operations
//! block, `deny_paths`, `coverage_semantics`, `rules` (clearable via
//! explicit `null`), `prune` — with `version` engine-managed. Candidate
//! records are validated against the medium-capability matrix
//! ([`crate::binding::validate_binding`]) before anything is written; only
//! refusals an edit *introduces* block it.

use std::path::Path;

use serde::Deserialize;

use crate::binding::{
    BINDING_VERSION, BindingV1, BuildMode, BuildOperation, CapabilityError, CoverageSemantics,
    Operations, PruneConfig, validate_binding,
};
use crate::engine::Engine;
use crate::ingest::resolve::resolve_binding;
use crate::pipeline::{Facet, IngestTrigger, Medium, Projection};
use crate::pipeline_store::{self, PipelineConfigs};
use crate::workspace_store::StoreError;

/// Failure modes of a pipeline edit. Distinct from the entity-centric
/// [`crate::engine::EngineError`] — these describe four-primitive store edits.
/// `key` is the display identity: `"<mem>/<name>"`.
#[derive(Debug, thiserror::Error)]
pub enum PipelineEditError {
    /// The engine was not booted from a workspace root, so there is no
    /// four-primitive store to edit (e.g. an engine built from a bare mount
    /// list in a test or in-memory consumer).
    #[error("engine has no workspace root — pipeline edits require a workspace-backed engine")]
    NoWorkspaceRoot,
    /// The edit landed on disk but its provenance record (the
    /// `__MEMSTEAD` mirror commit carrying the note) could not be
    /// committed. The disk state is live; the audit trail is missing
    /// this event — callers surface it rather than silently dropping
    /// the note.
    #[error("pipeline edit landed, but recording provenance failed: {0}")]
    Provenance(String),
    /// A create targeted a `(mem, name)` that already holds a record.
    #[error("{primitive} '{key}' already exists")]
    AlreadyExists {
        primitive: &'static str,
        key: String,
    },
    /// An update / delete / rename targeted a record that does not exist.
    #[error("{primitive} '{key}' does not exist")]
    NotFound {
        primitive: &'static str,
        key: String,
    },
    /// A delete was refused because other records still reference the target.
    #[error("{primitive} '{key}' is referenced by {referrers:?} — remove or repoint them first")]
    Referenced {
        primitive: &'static str,
        key: String,
        referrers: Vec<String>,
    },
    /// A rename target `(mem, new)` already holds a record.
    #[error("rename target {primitive} '{key}' already exists")]
    RenameTargetExists {
        primitive: &'static str,
        key: String,
    },
    /// A JSON-string edit entry point received a payload that did not
    /// deserialize into the target primitive.
    #[error("invalid {primitive} JSON: {message}")]
    InvalidJson {
        primitive: &'static str,
        message: String,
    },
    /// A binding edit was refused by the medium-capability matrix (D6) —
    /// e.g. declaring `sync` over a medium with no change signal. Carries
    /// only the refusals the edit would *introduce*: refusals the stored
    /// record already produces never block an unrelated edit. Each refusal
    /// message includes its remedy.
    #[error(
        "binding '{key}' edit refused by the medium-capability matrix: {}",
        format_refusals(refusals)
    )]
    Capability {
        key: String,
        refusals: Vec<CapabilityError>,
    },
    /// A binding edit would leave the record referencing a facet or medium
    /// that does not resolve — refused before anything is written.
    #[error("binding '{key}' edit refused — {message}")]
    Dangling { key: String, message: String },
    /// Underlying store IO / parse failure.
    #[error(transparent)]
    Store(#[from] StoreError),
}

fn key(mem: &str, name: &str) -> String {
    format!("{mem}/{name}")
}

fn format_refusals(refusals: &[CapabilityError]) -> String {
    refusals
        .iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

fn medium_exists(c: &PipelineConfigs, mem: &str, name: &str) -> bool {
    c.mediums.iter().any(|r| r.mem == mem && r.name == name)
}

fn facet_exists(c: &PipelineConfigs, mem: &str, name: &str) -> bool {
    c.facets.iter().any(|r| r.mem == mem && r.name == name)
}

fn projection_exists(c: &PipelineConfigs, mem: &str, name: &str) -> bool {
    c.projections.iter().any(|r| r.mem == mem && r.name == name)
}

/// Facet names (same mem) whose `medium` points at `name`.
fn facets_referencing_medium(c: &PipelineConfigs, mem: &str, name: &str) -> Vec<String> {
    c.facets
        .iter()
        .filter(|r| r.mem == mem && r.config.medium == name)
        .map(|r| r.name.clone())
        .collect()
}

/// Projection names (same mem) whose `source_facets` contain `name`.
fn projections_referencing_facet(c: &PipelineConfigs, mem: &str, name: &str) -> Vec<String> {
    c.projections
        .iter()
        .filter(|r| r.mem == mem && r.config.source_facets.iter().any(|f| f == name))
        .map(|r| r.name.clone())
        .collect()
}

/// Ingest names whose `projection` points at `<mem>/<name>`.
fn ingests_referencing_projection(c: &PipelineConfigs, mem: &str, name: &str) -> Vec<String> {
    let target = key(mem, name);
    c.ingests
        .iter()
        .filter(|r| r.config.projection == target)
        .map(|r| r.name.clone())
        .collect()
}

// --- Medium ----------------------------------------------------------------

/// Create a medium. Refuses if `(mem, name)` already holds one.
pub fn add_medium(
    root: &Path,
    mem: &str,
    name: &str,
    medium: &Medium,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if medium_exists(&configs, mem, name) {
        return Err(PipelineEditError::AlreadyExists {
            primitive: "medium",
            key: key(mem, name),
        });
    }
    pipeline_store::write_medium(root, mem, name, medium)?;
    Ok(())
}

/// Overwrite an existing medium. Refuses if `(mem, name)` does not exist.
pub fn update_medium(
    root: &Path,
    mem: &str,
    name: &str,
    medium: &Medium,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !medium_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "medium",
            key: key(mem, name),
        });
    }
    pipeline_store::write_medium(root, mem, name, medium)?;
    Ok(())
}

/// Delete a medium. Refuses if any facet in the same mem still references it.
pub fn delete_medium(root: &Path, mem: &str, name: &str) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !medium_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "medium",
            key: key(mem, name),
        });
    }
    let referrers = facets_referencing_medium(&configs, mem, name);
    if !referrers.is_empty() {
        return Err(PipelineEditError::Referenced {
            primitive: "medium",
            key: key(mem, name),
            referrers,
        });
    }
    pipeline_store::delete_medium(root, mem, name)?;
    Ok(())
}

/// Rename a medium within its mem, updating its embedded `name` and every
/// dependent facet's `medium` reference. No-op when `old == new`.
pub fn rename_medium(
    root: &Path,
    mem: &str,
    old: &str,
    new: &str,
) -> Result<(), PipelineEditError> {
    if old == new {
        return Ok(());
    }
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    let existing = configs
        .mediums
        .iter()
        .find(|r| r.mem == mem && r.name == old)
        .ok_or_else(|| PipelineEditError::NotFound {
            primitive: "medium",
            key: key(mem, old),
        })?;
    if medium_exists(&configs, mem, new) {
        return Err(PipelineEditError::RenameTargetExists {
            primitive: "medium",
            key: key(mem, new),
        });
    }
    let mut renamed = existing.config.clone();
    renamed.name = new.to_string();
    // Write the new stem first, then repoint referrers, then drop the old —
    // no point in the sequence leaves a facet pointing at a missing medium.
    pipeline_store::write_medium(root, mem, new, &renamed)?;
    for facet in facets_referencing_medium(&configs, mem, old) {
        if let Some(rec) = configs
            .facets
            .iter()
            .find(|r| r.mem == mem && r.name == facet)
        {
            let mut updated = rec.config.clone();
            updated.medium = new.to_string();
            pipeline_store::write_facet(root, mem, &facet, &updated)?;
        }
    }
    pipeline_store::delete_medium(root, mem, old)?;
    Ok(())
}

// --- Facet -----------------------------------------------------------------

/// Create a facet. Refuses if `(mem, name)` already holds one.
pub fn add_facet(
    root: &Path,
    mem: &str,
    name: &str,
    facet: &Facet,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if facet_exists(&configs, mem, name) {
        return Err(PipelineEditError::AlreadyExists {
            primitive: "facet",
            key: key(mem, name),
        });
    }
    pipeline_store::write_facet(root, mem, name, facet)?;
    Ok(())
}

/// Overwrite an existing facet. Refuses if `(mem, name)` does not exist.
pub fn update_facet(
    root: &Path,
    mem: &str,
    name: &str,
    facet: &Facet,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !facet_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "facet",
            key: key(mem, name),
        });
    }
    pipeline_store::write_facet(root, mem, name, facet)?;
    Ok(())
}

/// Delete a facet. Refuses if any projection in the same mem references it.
pub fn delete_facet(root: &Path, mem: &str, name: &str) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !facet_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "facet",
            key: key(mem, name),
        });
    }
    let referrers = projections_referencing_facet(&configs, mem, name);
    if !referrers.is_empty() {
        return Err(PipelineEditError::Referenced {
            primitive: "facet",
            key: key(mem, name),
            referrers,
        });
    }
    pipeline_store::delete_facet(root, mem, name)?;
    Ok(())
}

/// Rename a facet within its mem, updating its embedded `name` and every
/// dependent projection's `source_facets` entry. No-op when `old == new`.
pub fn rename_facet(root: &Path, mem: &str, old: &str, new: &str) -> Result<(), PipelineEditError> {
    if old == new {
        return Ok(());
    }
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    let existing = configs
        .facets
        .iter()
        .find(|r| r.mem == mem && r.name == old)
        .ok_or_else(|| PipelineEditError::NotFound {
            primitive: "facet",
            key: key(mem, old),
        })?;
    if facet_exists(&configs, mem, new) {
        return Err(PipelineEditError::RenameTargetExists {
            primitive: "facet",
            key: key(mem, new),
        });
    }
    let mut renamed = existing.config.clone();
    renamed.name = new.to_string();
    pipeline_store::write_facet(root, mem, new, &renamed)?;
    for proj in projections_referencing_facet(&configs, mem, old) {
        if let Some(rec) = configs
            .projections
            .iter()
            .find(|r| r.mem == mem && r.name == proj)
        {
            let mut updated = rec.config.clone();
            for f in updated.source_facets.iter_mut() {
                if f == old {
                    *f = new.to_string();
                }
            }
            pipeline_store::write_projection(root, mem, &proj, &updated)?;
        }
    }
    pipeline_store::delete_facet(root, mem, old)?;
    Ok(())
}

// --- Projection ------------------------------------------------------------

/// Create a projection. Refuses if `(mem, name)` already holds one.
pub fn add_projection(
    root: &Path,
    mem: &str,
    name: &str,
    projection: &Projection,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if projection_exists(&configs, mem, name) {
        return Err(PipelineEditError::AlreadyExists {
            primitive: "projection",
            key: key(mem, name),
        });
    }
    pipeline_store::write_projection(root, mem, name, projection)?;
    Ok(())
}

/// Overwrite an existing projection. Refuses if `(mem, name)` does not exist.
pub fn update_projection(
    root: &Path,
    mem: &str,
    name: &str,
    projection: &Projection,
) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !projection_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "projection",
            key: key(mem, name),
        });
    }
    pipeline_store::write_projection(root, mem, name, projection)?;
    Ok(())
}

/// Delete a projection. Refuses if any ingest still runs it.
pub fn delete_projection(root: &Path, mem: &str, name: &str) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !projection_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "projection",
            key: key(mem, name),
        });
    }
    let referrers = ingests_referencing_projection(&configs, mem, name);
    if !referrers.is_empty() {
        return Err(PipelineEditError::Referenced {
            primitive: "projection",
            key: key(mem, name),
            referrers,
        });
    }
    pipeline_store::delete_projection(root, mem, name)?;
    Ok(())
}

/// Rename a projection within its mem (a projection has no embedded name, so
/// the file moves), repointing every ingest whose `projection` was
/// `<mem>/<old>`. No-op when `old == new`.
pub fn rename_projection(
    root: &Path,
    mem: &str,
    old: &str,
    new: &str,
) -> Result<(), PipelineEditError> {
    if old == new {
        return Ok(());
    }
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !projection_exists(&configs, mem, old) {
        return Err(PipelineEditError::NotFound {
            primitive: "projection",
            key: key(mem, old),
        });
    }
    if projection_exists(&configs, mem, new) {
        return Err(PipelineEditError::RenameTargetExists {
            primitive: "projection",
            key: key(mem, new),
        });
    }
    pipeline_store::rename_projection(root, mem, old, new)?;
    let new_ref = key(mem, new);
    for ingest in ingests_referencing_projection(&configs, mem, old) {
        if let Some(rec) = configs.ingests.iter().find(|r| r.name == ingest) {
            let mut updated = rec.config.clone();
            updated.projection = new_ref.clone();
            pipeline_store::write_ingest(root, &ingest, &updated)?;
        }
    }
    Ok(())
}

// --- Binding record (JSON patch) --------------------------------------------

/// Deserialize helper distinguishing an absent field (outer `None` —
/// preserve) from an explicit `null` (inner `None` — clear). serde's plain
/// `Option<Option<T>>` collapses `null` to the outer `None`; wrapping the
/// parsed value in `Some` keeps the two cases apart.
fn patch_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A partial edit of a binding record, decoded from the JSON edit entry
/// points ([`update_binding_json`] / [`add_binding_json`]).
///
/// **Patch semantics**: a field absent from the payload is preserved from
/// the stored record — the preserve-operations guarantee the old
/// projection-overlay carried extends to every field. A field present is
/// applied. The natively-optional record fields (`intent`, `rules`,
/// `prune`) additionally distinguish explicit `null` (clear) from absence
/// (preserve) — this is what makes `rules` clearable, which the old
/// set-only overlay could not do. `operations` is replaced as a whole
/// block when present: the block is the unit of operations authoring, so
/// an op absent from a supplied block is removed (legal — an absent
/// mutating op refuses at run time with its enable remedy).
///
/// `version` is engine-managed and never author input: a `version` key in
/// the payload is ignored, like every unknown key (the format grows
/// additively — tolerance is deliberate).
#[derive(Debug, Default, Deserialize)]
pub struct BindingPatch {
    /// Set / clear (`null`) / preserve (absent) the binding's intent prose.
    #[serde(default, deserialize_with = "patch_field")]
    pub intent: Option<Option<String>>,
    /// Replace the source facet list when present.
    #[serde(default)]
    pub source_facets: Option<Vec<String>>,
    /// Replace the read-only reference-mem list when present.
    #[serde(default)]
    pub reference_mems: Option<Vec<String>>,
    /// Repoint the destination mem when present (`null` preserves — a
    /// required record field cannot be cleared).
    #[serde(default)]
    pub destination_mem: Option<String>,
    /// Replace the deny-path glob list when present.
    #[serde(default)]
    pub deny_paths: Option<Vec<String>>,
    /// Replace the coverage claim when present.
    #[serde(default)]
    pub coverage_semantics: Option<CoverageSemantics>,
    /// Set / clear (`null`) / preserve (absent) the free-form rules value.
    #[serde(default, deserialize_with = "patch_field")]
    pub rules: Option<Option<serde_json::Value>>,
    /// Set / clear (`null`) / preserve (absent) the prune policy.
    #[serde(default, deserialize_with = "patch_field")]
    pub prune: Option<Option<PruneConfig>>,
    /// Replace the whole operations block when present.
    #[serde(default)]
    pub operations: Option<Operations>,
}

impl BindingPatch {
    /// Overlay this patch onto `binding` (patch semantics; `version`
    /// untouched).
    fn apply(self, binding: &mut BindingV1) {
        if let Some(v) = self.intent {
            binding.intent = v;
        }
        if let Some(v) = self.source_facets {
            binding.source_facets = v;
        }
        if let Some(v) = self.reference_mems {
            binding.reference_mems = v;
        }
        if let Some(v) = self.destination_mem {
            binding.destination_mem = v;
        }
        if let Some(v) = self.deny_paths {
            binding.deny_paths = v;
        }
        if let Some(v) = self.coverage_semantics {
            binding.coverage_semantics = v;
        }
        if let Some(v) = self.rules {
            binding.rules = v;
        }
        if let Some(v) = self.prune {
            binding.prune = v;
        }
        if let Some(v) = self.operations {
            binding.operations = v;
        }
    }
}

/// The default scaffold a fresh binding is patched onto (D14): version
/// pinned, a default `build` block (discovery / loop / batch 20), sync and
/// verify absent, everything else empty — the caller's patch overlays it.
fn default_binding_scaffold() -> BindingV1 {
    BindingV1 {
        version: BINDING_VERSION,
        intent: None,
        source_facets: Vec::new(),
        reference_mems: Vec::new(),
        destination_mem: String::new(),
        deny_paths: Vec::new(),
        coverage_semantics: CoverageSemantics::default(),
        rules: None,
        prune: None,
        operations: Operations {
            build: Some(BuildOperation {
                mode: BuildMode::Discovery,
                trigger: IngestTrigger::Loop,
                batch_size: 20,
                post_actions: None,
            }),
            sync: None,
            verify: None,
        },
    }
}

/// The capability refusals a candidate binding produces against the loaded
/// configs (`Ok(vec![])` = clean), or the resolve error message when a
/// facet / medium reference does not resolve.
fn capability_refusals(
    configs: &PipelineConfigs,
    binding_id: &str,
    binding: &BindingV1,
) -> Result<Vec<CapabilityError>, String> {
    let resolved = resolve_binding(configs, binding_id, binding).map_err(|e| e.to_string())?;
    Ok(validate_binding(&resolved).err().unwrap_or_default())
}

/// Create a binding from a JSON [`BindingPatch`] applied to the default
/// scaffold. Refuses when `(mem, name)` already holds a record
/// ([`PipelineEditError::AlreadyExists`]), when the patch leaves
/// `destination_mem` empty ([`PipelineEditError::InvalidJson`]), when a
/// referenced facet / medium does not resolve
/// ([`PipelineEditError::Dangling`]), or when the candidate fails the
/// medium-capability matrix ([`PipelineEditError::Capability`] — every
/// refusal blocks; a fresh record has no pre-existing state to
/// grandfather). Returns the written record.
pub fn add_binding_json(
    root: &Path,
    mem: &str,
    name: &str,
    patch_json: &str,
) -> Result<BindingV1, PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if projection_exists(&configs, mem, name) {
        return Err(PipelineEditError::AlreadyExists {
            primitive: "projection",
            key: key(mem, name),
        });
    }
    let patch: BindingPatch = parse_json(patch_json, "projection")?;
    let mut binding = default_binding_scaffold();
    patch.apply(&mut binding);
    if binding.destination_mem.is_empty() {
        return Err(PipelineEditError::InvalidJson {
            primitive: "projection",
            message: "destination_mem is required".to_string(),
        });
    }
    let binding_id = key(mem, name);
    match capability_refusals(&configs, &binding_id, &binding) {
        Ok(refusals) if !refusals.is_empty() => {
            return Err(PipelineEditError::Capability {
                key: binding_id,
                refusals,
            });
        }
        Err(message) => {
            return Err(PipelineEditError::Dangling {
                key: binding_id,
                message,
            });
        }
        Ok(_) => {}
    }
    pipeline_store::write_binding(root, mem, name, &binding)?;
    Ok(binding)
}

/// Patch an existing binding from a JSON [`BindingPatch`] — the projection
/// update path widened to the full author-editable record (operations,
/// `deny_paths`, `coverage_semantics`, `rules` including clearing,
/// `prune`). Absent fields are preserved; explicit `null` clears `intent`
/// / `rules` / `prune`; a present `operations` block replaces the whole
/// block; `version` stays engine-managed.
///
/// Refuses when the record does not exist
/// ([`PipelineEditError::NotFound`]), when the patch introduces a dangling
/// facet / medium reference ([`PipelineEditError::Dangling`]), or when it
/// introduces a capability refusal the stored record did not already carry
/// ([`PipelineEditError::Capability`] — pre-existing refusals never block
/// an unrelated edit). Nothing is written on refusal. Returns the written
/// record.
pub fn update_binding_json(
    root: &Path,
    mem: &str,
    name: &str,
    patch_json: &str,
) -> Result<BindingV1, PipelineEditError> {
    let configs = pipeline_store::load_legacy_pipeline_configs(root)?;
    if !projection_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "projection",
            key: key(mem, name),
        });
    }
    let patch: BindingPatch = parse_json(patch_json, "projection")?;
    let existing = pipeline_store::read_binding(root, mem, name)?;
    let mut patched = existing.clone();
    patch.apply(&mut patched);
    let binding_id = key(mem, name);
    match capability_refusals(&configs, &binding_id, &patched) {
        Ok(refusals) => {
            let before = capability_refusals(&configs, &binding_id, &existing).unwrap_or_default();
            let introduced: Vec<CapabilityError> = refusals
                .into_iter()
                .filter(|r| !before.contains(r))
                .collect();
            if !introduced.is_empty() {
                return Err(PipelineEditError::Capability {
                    key: binding_id,
                    refusals: introduced,
                });
            }
        }
        Err(message) => {
            // The candidate does not resolve. If the stored record does,
            // the patch introduced the dangling reference — refuse. If the
            // stored record is equally unresolvable, the brokenness is
            // pre-existing and an unrelated edit (e.g. repairing intent)
            // must not be blocked by it.
            if capability_refusals(&configs, &binding_id, &existing).is_ok() {
                return Err(PipelineEditError::Dangling {
                    key: binding_id,
                    message,
                });
            }
        }
    }
    pipeline_store::write_binding(root, mem, name, &patched)?;
    Ok(patched)
}

// --- Engine surface --------------------------------------------------------
//
// Thin wrappers that route an edit through the free functions above (disk +
// referential integrity) and then refresh the engine's in-memory
// `pipeline_configs` snapshot so a subsequent `pipeline_configs()` read sees
// the change. They use only the engine's public accessors, so this block stays
// out of the `engine` module's internals.

impl Engine {
    fn pipeline_edit_root(&self) -> Result<std::path::PathBuf, PipelineEditError> {
        self.workspace_root()
            .map(Path::to_path_buf)
            .ok_or(PipelineEditError::NoWorkspaceRoot)
    }

    fn refresh_pipeline_configs(&mut self, root: &Path) -> Result<(), PipelineEditError> {
        // The referential-integrity edit layer operates on the four-primitive
        // (legacy) shape — the version-gated binding loader is the live brief
        // path's, not the editor's.
        self.set_pipeline_configs(pipeline_store::load_legacy_pipeline_configs(root)?);
        Ok(())
    }

    /// Provenance bridge with error translation: the disk write has
    /// already landed when this runs, so a failure here is surfaced as
    /// `Provenance` (edit live, audit record missing) rather than
    /// rolling anything back.
    fn pipeline_provenance(
        &self,
        mem: &str,
        kind: &str,
        edits: &[(String, Option<Vec<u8>>)],
        note: Option<&str>,
        verb: &str,
    ) -> Result<(), PipelineEditError> {
        self.record_pipeline_edit_provenance(mem, kind, edits, note, verb)
            .map_err(|e| PipelineEditError::Provenance(e.to_string()))
    }

    /// Create a medium and refresh the in-memory snapshot. See [`add_medium`].
    pub fn add_medium(
        &mut self,
        mem: &str,
        name: &str,
        medium: &Medium,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        add_medium(&root, mem, name, medium)?;
        let bytes =
            serde_json::to_vec_pretty(medium).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "mediums",
            &[(name.to_string(), Some(bytes))],
            note,
            "add",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Overwrite a medium and refresh the snapshot. See [`update_medium`].
    pub fn update_medium(
        &mut self,
        mem: &str,
        name: &str,
        medium: &Medium,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        update_medium(&root, mem, name, medium)?;
        let bytes =
            serde_json::to_vec_pretty(medium).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "mediums",
            &[(name.to_string(), Some(bytes))],
            note,
            "update",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Delete a medium and refresh the snapshot. See [`delete_medium`].
    pub fn delete_medium(
        &mut self,
        mem: &str,
        name: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        delete_medium(&root, mem, name)?;
        self.pipeline_provenance(mem, "mediums", &[(name.to_string(), None)], note, "delete")?;
        self.refresh_pipeline_configs(&root)
    }

    /// Rename a medium and refresh the snapshot. See [`rename_medium`].
    pub fn rename_medium(
        &mut self,
        mem: &str,
        old: &str,
        new: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        rename_medium(&root, mem, old, new)?;
        // Mirror the rename as remove-old + upsert-new in one commit.
        // The in-memory snapshot still holds the record under its old
        // name here (refresh runs after), and a rename changes the
        // name only — the config bytes are those of the old record.
        let bytes = self
            .pipeline_configs()
            .mediums
            .iter()
            .find(|r| r.mem == mem && r.name == old)
            .map(|r| serde_json::to_vec_pretty(&r.config))
            .transpose()
            .map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "mediums",
            &[(old.to_string(), None), (new.to_string(), bytes)],
            note,
            "rename",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Create a facet and refresh the snapshot. See [`add_facet`].
    pub fn add_facet(
        &mut self,
        mem: &str,
        name: &str,
        facet: &Facet,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        add_facet(&root, mem, name, facet)?;
        let bytes =
            serde_json::to_vec_pretty(facet).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "facets",
            &[(name.to_string(), Some(bytes))],
            note,
            "add",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Overwrite a facet and refresh the snapshot. See [`update_facet`].
    pub fn update_facet(
        &mut self,
        mem: &str,
        name: &str,
        facet: &Facet,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        update_facet(&root, mem, name, facet)?;
        let bytes =
            serde_json::to_vec_pretty(facet).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "facets",
            &[(name.to_string(), Some(bytes))],
            note,
            "update",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Delete a facet and refresh the snapshot. See [`delete_facet`].
    pub fn delete_facet(
        &mut self,
        mem: &str,
        name: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        delete_facet(&root, mem, name)?;
        self.pipeline_provenance(mem, "facets", &[(name.to_string(), None)], note, "delete")?;
        self.refresh_pipeline_configs(&root)
    }

    /// Rename a facet and refresh the snapshot. See [`rename_facet`].
    pub fn rename_facet(
        &mut self,
        mem: &str,
        old: &str,
        new: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        rename_facet(&root, mem, old, new)?;
        // Mirror the rename as remove-old + upsert-new in one commit.
        // The in-memory snapshot still holds the record under its old
        // name here (refresh runs after), and a rename changes the
        // name only — the config bytes are those of the old record.
        let bytes = self
            .pipeline_configs()
            .facets
            .iter()
            .find(|r| r.mem == mem && r.name == old)
            .map(|r| serde_json::to_vec_pretty(&r.config))
            .transpose()
            .map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "facets",
            &[(old.to_string(), None), (new.to_string(), bytes)],
            note,
            "rename",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Create a projection and refresh the snapshot. See [`add_projection`].
    pub fn add_projection(
        &mut self,
        mem: &str,
        name: &str,
        projection: &Projection,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        add_projection(&root, mem, name, projection)?;
        let bytes =
            serde_json::to_vec_pretty(projection).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "projections",
            &[(name.to_string(), Some(bytes))],
            note,
            "add",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Overwrite a projection and refresh the snapshot. See [`update_projection`].
    pub fn update_projection(
        &mut self,
        mem: &str,
        name: &str,
        projection: &Projection,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        update_projection(&root, mem, name, projection)?;
        let bytes =
            serde_json::to_vec_pretty(projection).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "projections",
            &[(name.to_string(), Some(bytes))],
            note,
            "update",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Delete a projection and refresh the snapshot. See [`delete_projection`].
    pub fn delete_projection(
        &mut self,
        mem: &str,
        name: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        delete_projection(&root, mem, name)?;
        self.pipeline_provenance(
            mem,
            "projections",
            &[(name.to_string(), None)],
            note,
            "delete",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Rename a projection and refresh the snapshot. See [`rename_projection`].
    pub fn rename_projection(
        &mut self,
        mem: &str,
        old: &str,
        new: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        rename_projection(&root, mem, old, new)?;
        // Mirror the rename as remove-old + upsert-new in one commit.
        // The in-memory snapshot still holds the record under its old
        // name here (refresh runs after), and a rename changes the
        // name only — the config bytes are those of the old record.
        let bytes = self
            .pipeline_configs()
            .projections
            .iter()
            .find(|r| r.mem == mem && r.name == old)
            .map(|r| serde_json::to_vec_pretty(&r.config))
            .transpose()
            .map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "projections",
            &[(old.to_string(), None), (new.to_string(), bytes)],
            note,
            "rename",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    // JSON-string entry points for serialization-boundary callers (UniFFI,
    // CLI) that carry a primitive as JSON rather than a typed value. They
    // deserialize here — where serde already lives — and delegate to the
    // typed methods above, so the FFI translation layer needs no JSON
    // dependency of its own. Only `add`/`update` carry a payload; `delete`
    // and `rename` take plain string identifiers and use the typed methods
    // directly.

    /// [`Self::add_medium`] from a JSON-encoded [`Medium`].
    pub fn add_medium_json(
        &mut self,
        mem: &str,
        name: &str,
        medium_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.add_medium(mem, name, &parse_json(medium_json, "medium")?, note)
    }

    /// [`Self::update_medium`] from a JSON-encoded [`Medium`].
    pub fn update_medium_json(
        &mut self,
        mem: &str,
        name: &str,
        medium_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.update_medium(mem, name, &parse_json(medium_json, "medium")?, note)
    }

    /// [`Self::add_facet`] from a JSON-encoded [`Facet`].
    pub fn add_facet_json(
        &mut self,
        mem: &str,
        name: &str,
        facet_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.add_facet(mem, name, &parse_json(facet_json, "facet")?, note)
    }

    /// [`Self::update_facet`] from a JSON-encoded [`Facet`].
    pub fn update_facet_json(
        &mut self,
        mem: &str,
        name: &str,
        facet_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        self.update_facet(mem, name, &parse_json(facet_json, "facet")?, note)
    }

    /// Create a binding from a JSON [`BindingPatch`] applied to the default
    /// scaffold (D14 widened): the caller may supply any author-editable
    /// field, including a full `operations` block; an absent block scaffolds
    /// the default `build` (discovery / loop / batch 20). See
    /// [`add_binding_json`] for the refusals (duplicate, missing
    /// `destination_mem`, dangling reference, capability matrix).
    pub fn add_projection_json(
        &mut self,
        mem: &str,
        name: &str,
        projection_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        let binding = add_binding_json(&root, mem, name, projection_json)?;
        self.record_binding_edit(mem, name, &binding, &root, note, "add")
    }

    /// Patch a binding from a JSON [`BindingPatch`] — the projection update
    /// path widened to the full author-editable record (D14). Absent fields
    /// are preserved (the preserve-operations property, extended to every
    /// field); explicit `null` clears `intent` / `rules` / `prune`; a present
    /// `operations` block replaces the whole block; `version` stays
    /// engine-managed. See [`update_binding_json`] for the refusals
    /// (not-found, dangling reference, capability matrix).
    pub fn update_projection_json(
        &mut self,
        mem: &str,
        name: &str,
        projection_json: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        let binding = update_binding_json(&root, mem, name, projection_json)?;
        self.record_binding_edit(mem, name, &binding, &root, note, "update")
    }

    /// Shared provenance + refresh tail for the binding edit path: the disk
    /// write has already landed in the free function; record the edit's
    /// mirror commit and refresh the in-memory snapshot.
    fn record_binding_edit(
        &mut self,
        mem: &str,
        name: &str,
        binding: &BindingV1,
        root: &Path,
        note: Option<&str>,
        verb: &str,
    ) -> Result<(), PipelineEditError> {
        let bytes =
            serde_json::to_vec_pretty(binding).map_err(|e| PipelineEditError::InvalidJson {
                primitive: "config",
                message: e.to_string(),
            })?;
        self.pipeline_provenance(
            mem,
            "projections",
            &[(name.to_string(), Some(bytes))],
            note,
            verb,
        )?;
        self.refresh_pipeline_configs(root)
    }
}

/// Deserialize a pipeline primitive from JSON, mapping a parse failure to a
/// typed [`PipelineEditError::InvalidJson`] naming the primitive.
fn parse_json<T: serde::de::DeserializeOwned>(
    json: &str,
    primitive: &'static str,
) -> Result<T, PipelineEditError> {
    serde_json::from_str(json).map_err(|e| PipelineEditError::InvalidJson {
        primitive,
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{IngestTrigger, MediumType, PatternEntry, PatternMode};
    use crate::pipeline_store::{LegacyIngest, LegacyIngestMode};
    use tempfile::TempDir;

    fn medium(name: &str) -> Medium {
        Medium {
            name: name.to_string(),
            medium_type: MediumType::Codebase,
            pointer: "../src".to_string(),
            change_detection: None,
        }
    }

    fn facet(name: &str, medium: &str) -> Facet {
        Facet {
            name: name.to_string(),
            medium: medium.to_string(),
            scope: vec![PatternEntry {
                path: "**/*.rs".to_string(),
                mode: PatternMode::Allow,
            }],
            engagement: None,
            preparation: None,
        }
    }

    fn projection(facets: &[&str]) -> Projection {
        Projection {
            intent: Some("test".to_string()),
            source_facets: facets.iter().map(|s| s.to_string()).collect(),
            reference_mems: vec![],
            destination_mem: "v".to_string(),
            rules: None,
        }
    }

    fn ingest(projection: &str) -> LegacyIngest {
        LegacyIngest {
            projection: projection.to_string(),
            mode: LegacyIngestMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 10,
            deny_paths: vec![],
            post_actions: None,
        }
    }

    #[test]
    fn add_then_duplicate_medium_refuses() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "m", &medium("m")).unwrap();
        let err = add_medium(root, "v", "m", &medium("m")).unwrap_err();
        assert!(
            matches!(err, PipelineEditError::AlreadyExists { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn update_missing_medium_refuses() {
        let tmp = TempDir::new().unwrap();
        let err = update_medium(tmp.path(), "v", "m", &medium("m")).unwrap_err();
        assert!(
            matches!(err, PipelineEditError::NotFound { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn delete_medium_refused_while_a_facet_references_it() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "m", &medium("m")).unwrap();
        add_facet(root, "v", "f", &facet("f", "m")).unwrap();

        let err = delete_medium(root, "v", "m").unwrap_err();
        match err {
            PipelineEditError::Referenced { referrers, .. } => assert_eq!(referrers, vec!["f"]),
            other => panic!("expected Referenced, got {other:?}"),
        }
        // Removing the facet frees the medium.
        delete_facet(root, "v", "f").unwrap();
        delete_medium(root, "v", "m").unwrap();
        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert!(configs.mediums.is_empty() && configs.facets.is_empty());
    }

    #[test]
    fn rename_medium_repoints_dependent_facets_and_updates_embedded_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "old", &medium("old")).unwrap();
        add_facet(root, "v", "f", &facet("f", "old")).unwrap();

        rename_medium(root, "v", "old", "new").unwrap();

        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.mediums.len(), 1);
        assert_eq!(configs.mediums[0].name, "new");
        // Embedded name tracks the stem.
        assert_eq!(configs.mediums[0].config.name, "new");
        // Dependent facet now points at the new medium name.
        assert_eq!(configs.facets[0].config.medium, "new");
    }

    #[test]
    fn rename_medium_refuses_existing_target() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "a", &medium("a")).unwrap();
        add_medium(root, "v", "b", &medium("b")).unwrap();
        let err = rename_medium(root, "v", "a", "b").unwrap_err();
        assert!(
            matches!(err, PipelineEditError::RenameTargetExists { .. }),
            "got {err:?}"
        );
        // Nothing lost.
        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.mediums.len(), 2);
    }

    #[test]
    fn rename_medium_to_same_name_is_a_noop() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "m", &medium("m")).unwrap();
        rename_medium(root, "v", "m", "m").unwrap();
        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.mediums.len(), 1);
        assert_eq!(configs.mediums[0].config, medium("m"));
    }

    #[test]
    fn delete_facet_refused_while_a_projection_references_it() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_facet(root, "v", "f", &facet("f", "m")).unwrap();
        add_projection(root, "v", "p", &projection(&["f"])).unwrap();
        let err = delete_facet(root, "v", "f").unwrap_err();
        assert!(
            matches!(err, PipelineEditError::Referenced { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn rename_facet_repoints_dependent_projections() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_facet(root, "v", "old", &facet("old", "m")).unwrap();
        add_projection(root, "v", "p", &projection(&["old", "other"])).unwrap();

        rename_facet(root, "v", "old", "new").unwrap();

        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.facets[0].name, "new");
        assert_eq!(configs.facets[0].config.name, "new");
        assert_eq!(
            configs.projections[0].config.source_facets,
            vec!["new", "other"]
        );
    }

    #[test]
    fn delete_projection_refused_while_an_ingest_runs_it() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_projection(root, "v", "p", &projection(&[])).unwrap();
        pipeline_store::write_ingest(root, "i", &ingest("v/p")).unwrap();
        let err = delete_projection(root, "v", "p").unwrap_err();
        match err {
            PipelineEditError::Referenced { referrers, .. } => assert_eq!(referrers, vec!["i"]),
            other => panic!("expected Referenced, got {other:?}"),
        }
    }

    #[test]
    fn parse_json_accepts_a_valid_medium() {
        let m: Medium =
            parse_json(r#"{"name":"m","type":"codebase","pointer":".."}"#, "medium").unwrap();
        assert_eq!(m.name, "m");
        assert_eq!(m.medium_type, MediumType::Codebase);
    }

    #[test]
    fn parse_json_maps_a_bad_payload_to_invalid_json() {
        let err = parse_json::<Medium>("{ not json", "medium").unwrap_err();
        assert!(
            matches!(
                err,
                PipelineEditError::InvalidJson {
                    primitive: "medium",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn rename_projection_repoints_dependent_ingests() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_projection(root, "v", "old", &projection(&[])).unwrap();
        pipeline_store::write_ingest(root, "i", &ingest("v/old")).unwrap();

        rename_projection(root, "v", "old", "new").unwrap();

        let configs = pipeline_store::load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.projections[0].name, "new");
        assert_eq!(configs.ingests[0].config.projection, "v/new");
    }

    // ---- binding record (JSON patch, D14 widened) -------------------------

    use crate::binding::{PruneGuarantee, SyncOperation, VerifyOperation};

    fn web_medium(name: &str) -> Medium {
        Medium {
            name: name.to_string(),
            medium_type: MediumType::Web,
            pointer: "https://example.com".to_string(),
            change_detection: None,
        }
    }

    /// A root with a codebase medium `m` + facet `f` and a web medium `w` +
    /// facet `wf`, all in mem `v`.
    fn binding_fixture() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_medium(root, "v", "m", &medium("m")).unwrap();
        add_facet(root, "v", "f", &facet("f", "m")).unwrap();
        add_medium(root, "v", "w", &web_medium("w")).unwrap();
        add_facet(root, "v", "wf", &facet("wf", "w")).unwrap();
        tmp
    }

    /// The old 5-field projection payload still creates a binding, scaffolded
    /// with the default build block — the widened decode does not break the
    /// projection-level callers.
    #[test]
    fn add_binding_json_scaffolds_default_build_for_projection_payload() {
        let tmp = binding_fixture();
        let root = tmp.path();
        let b = add_binding_json(
            root,
            "v",
            "p",
            r#"{"intent":"i","source_facets":["f"],"reference_mems":[],"destination_mem":"v","rules":{"routing":"r"}}"#,
        )
        .unwrap();
        assert_eq!(b.version, BINDING_VERSION);
        let build = b.operations.build.as_ref().unwrap();
        assert_eq!(build.mode, BuildMode::Discovery);
        assert_eq!(build.batch_size, 20);
        assert!(b.operations.sync.is_none() && b.operations.verify.is_none());
        assert_eq!(b.rules, Some(serde_json::json!({ "routing": "r" })));
        assert_eq!(pipeline_store::read_binding(root, "v", "p").unwrap(), b);
    }

    /// A payload declaring the full record — operations block, deny_paths,
    /// coverage, prune — is written as given (the scaffold is only a fallback).
    #[test]
    fn add_binding_json_accepts_the_full_record() {
        let tmp = binding_fixture();
        let root = tmp.path();
        let b = add_binding_json(
            root,
            "v",
            "p",
            r#"{
              "source_facets": ["f"],
              "destination_mem": "v",
              "deny_paths": ["dev/**"],
              "coverage_semantics": "curated",
              "prune": { "guarantee": "never-clobber" },
              "operations": {
                "build": { "mode": "one-shot", "trigger": "manual", "batch_size": 5 },
                "sync": { "trigger": "manual", "batch_size": 7 }
              }
            }"#,
        )
        .unwrap();
        assert_eq!(b.operations.build.as_ref().unwrap().mode, BuildMode::OneShot);
        assert_eq!(b.operations.sync.as_ref().unwrap().batch_size, 7);
        assert!(b.operations.verify.is_none());
        assert_eq!(b.deny_paths, vec!["dev/**"]);
        assert_eq!(b.coverage_semantics, CoverageSemantics::Curated);
        assert_eq!(
            b.prune.as_ref().unwrap().guarantee,
            PruneGuarantee::NeverClobber
        );
    }

    #[test]
    fn add_binding_json_refuses_duplicate() {
        let tmp = binding_fixture();
        let root = tmp.path();
        let payload = r#"{"source_facets":["f"],"destination_mem":"v"}"#;
        add_binding_json(root, "v", "p", payload).unwrap();
        let err = add_binding_json(root, "v", "p", payload).unwrap_err();
        assert!(
            matches!(err, PipelineEditError::AlreadyExists { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn add_binding_json_requires_destination_mem() {
        let tmp = binding_fixture();
        let root = tmp.path();
        let err = add_binding_json(root, "v", "p", r#"{"source_facets":["f"]}"#).unwrap_err();
        match err {
            PipelineEditError::InvalidJson { message, .. } => {
                assert!(message.contains("destination_mem"), "got: {message}")
            }
            other => panic!("expected InvalidJson, got {other:?}"),
        }
    }

    #[test]
    fn add_binding_json_refuses_dangling_facet() {
        let tmp = binding_fixture();
        let root = tmp.path();
        let err = add_binding_json(
            root,
            "v",
            "p",
            r#"{"source_facets":["missing"],"destination_mem":"v"}"#,
        )
        .unwrap_err();
        assert!(
            matches!(err, PipelineEditError::Dangling { .. }),
            "got {err:?}"
        );
        assert!(pipeline_store::read_binding(root, "v", "p").is_err());
    }

    /// D6 at the edit seam: declaring sync over a web (change-signal-less)
    /// medium refuses at add time with the matrix's typed message.
    #[test]
    fn add_binding_json_refuses_capability_violation() {
        let tmp = binding_fixture();
        let root = tmp.path();
        let err = add_binding_json(
            root,
            "v",
            "p",
            r#"{
              "source_facets": ["wf"],
              "destination_mem": "v",
              "operations": {
                "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 },
                "sync": { "trigger": "manual", "batch_size": 20 }
              }
            }"#,
        )
        .unwrap_err();
        match &err {
            PipelineEditError::Capability { refusals, .. } => {
                assert!(
                    refusals.iter().any(|r| matches!(
                        r,
                        CapabilityError::OperationOutOfScope { operation, .. } if *operation == "sync"
                    )),
                    "expected an OperationOutOfScope(sync) refusal, got {refusals:?}"
                );
            }
            other => panic!("expected Capability, got {other:?}"),
        }
        assert!(pipeline_store::read_binding(root, "v", "p").is_err());
    }

    /// The full record used by the update tests: build+sync+verify over the
    /// codebase facet, deny_paths, curated coverage, rules, prune.
    fn full_binding_payload() -> &'static str {
        r#"{
          "intent": "i",
          "source_facets": ["f"],
          "reference_mems": ["r"],
          "destination_mem": "v",
          "deny_paths": ["dev/**"],
          "coverage_semantics": "curated",
          "rules": { "routing": "r" },
          "prune": { "guarantee": "never-clobber" },
          "operations": {
            "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 },
            "sync": { "trigger": "manual", "batch_size": 20 },
            "verify": { "trigger": "manual", "batch_size": 20 }
          }
        }"#
    }

    /// Patch semantics: a single-field patch preserves every sibling field —
    /// the preserve-operations property, extended to the full record.
    #[test]
    fn update_binding_json_patch_preserves_untouched_fields() {
        let tmp = binding_fixture();
        let root = tmp.path();
        let before = add_binding_json(root, "v", "p", full_binding_payload()).unwrap();
        let after = update_binding_json(root, "v", "p", r#"{"intent":"new"}"#).unwrap();
        assert_eq!(after.intent.as_deref(), Some("new"));
        assert_eq!(after.version, before.version);
        assert_eq!(after.source_facets, before.source_facets);
        assert_eq!(after.reference_mems, before.reference_mems);
        assert_eq!(after.destination_mem, before.destination_mem);
        assert_eq!(after.deny_paths, before.deny_paths);
        assert_eq!(after.coverage_semantics, before.coverage_semantics);
        assert_eq!(after.rules, before.rules);
        assert_eq!(after.prune, before.prune);
        assert_eq!(after.operations, before.operations);
    }

    /// Explicit `null` clears the natively-optional fields; absence preserves
    /// them — the set-only rules asymmetry is gone.
    #[test]
    fn update_binding_json_null_clears_and_absence_preserves() {
        let tmp = binding_fixture();
        let root = tmp.path();
        add_binding_json(root, "v", "p", full_binding_payload()).unwrap();

        // Absent rules/prune/intent → preserved.
        let untouched = update_binding_json(root, "v", "p", r#"{"deny_paths":[]}"#).unwrap();
        assert!(untouched.rules.is_some() && untouched.prune.is_some());
        assert_eq!(untouched.intent.as_deref(), Some("i"));
        assert!(untouched.deny_paths.is_empty());

        // Explicit null → cleared.
        let cleared =
            update_binding_json(root, "v", "p", r#"{"rules": null, "prune": null}"#).unwrap();
        assert!(cleared.rules.is_none() && cleared.prune.is_none());
        assert_eq!(cleared.intent.as_deref(), Some("i"), "intent untouched");
        assert_eq!(
            pipeline_store::read_binding(root, "v", "p").unwrap(),
            cleared
        );
    }

    /// A present `operations` block replaces the whole block: ops absent from
    /// the supplied block are removed (legal — refusal happens at run time).
    #[test]
    fn update_binding_json_replaces_the_operations_block() {
        let tmp = binding_fixture();
        let root = tmp.path();
        add_binding_json(root, "v", "p", full_binding_payload()).unwrap();
        let after = update_binding_json(
            root,
            "v",
            "p",
            r#"{"operations": { "build": { "mode": "discovery", "trigger": "manual", "batch_size": 9 } }}"#,
        )
        .unwrap();
        assert_eq!(after.operations.build.as_ref().unwrap().batch_size, 9);
        assert!(after.operations.sync.is_none(), "sync removed with the block");
        assert!(after.operations.verify.is_none(), "verify removed with the block");
        assert_eq!(after.rules, Some(serde_json::json!({ "routing": "r" })));
    }

    /// D6 at the update seam: a patch that would declare sync over a web
    /// medium refuses, and the stored record stays byte-identical.
    #[test]
    fn update_binding_json_refuses_introduced_capability_violation() {
        let tmp = binding_fixture();
        let root = tmp.path();
        let before = add_binding_json(
            root,
            "v",
            "p",
            r#"{"source_facets":["wf"],"destination_mem":"v"}"#,
        )
        .unwrap();
        let err = update_binding_json(
            root,
            "v",
            "p",
            r#"{"operations": {
              "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 },
              "sync": { "trigger": "manual", "batch_size": 20 }
            }}"#,
        )
        .unwrap_err();
        assert!(
            matches!(err, PipelineEditError::Capability { .. }),
            "got {err:?}"
        );
        assert_eq!(
            pipeline_store::read_binding(root, "v", "p").unwrap(),
            before,
            "record unchanged on refusal"
        );
    }

    /// A refusal the stored record already produces never blocks an unrelated
    /// edit — pre-existing config is not this edit's to answer for.
    #[test]
    fn update_binding_json_allows_edit_despite_preexisting_refusal() {
        let tmp = binding_fixture();
        let root = tmp.path();
        // Bypass validation: a sync-over-web binding already on disk.
        let broken = BindingV1 {
            version: BINDING_VERSION,
            intent: None,
            source_facets: vec!["wf".to_string()],
            reference_mems: vec![],
            destination_mem: "v".to_string(),
            deny_paths: vec![],
            coverage_semantics: CoverageSemantics::Exhaustive,
            rules: None,
            prune: None,
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Loop,
                    batch_size: 20,
                    post_actions: None,
                }),
                sync: Some(SyncOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                }),
                verify: Some(VerifyOperation {
                    trigger: IngestTrigger::Manual,
                    batch_size: 20,
                    adjudication_cap: 50,
                    full_resync_every: 20,
                }),
            },
        };
        pipeline_store::write_binding(root, "v", "p", &broken).unwrap();

        let after = update_binding_json(root, "v", "p", r#"{"intent":"fixed"}"#).unwrap();
        assert_eq!(after.intent.as_deref(), Some("fixed"));
        assert!(after.operations.sync.is_some(), "pre-existing sync survives");
    }

    /// A patch introducing a dangling facet reference refuses; a patch that
    /// leaves pre-existing dangling references untouched does not.
    #[test]
    fn update_binding_json_dangling_reference_discipline() {
        let tmp = binding_fixture();
        let root = tmp.path();
        add_binding_json(
            root,
            "v",
            "p",
            r#"{"source_facets":["f"],"destination_mem":"v"}"#,
        )
        .unwrap();
        let err =
            update_binding_json(root, "v", "p", r#"{"source_facets":["missing"]}"#).unwrap_err();
        assert!(
            matches!(err, PipelineEditError::Dangling { .. }),
            "got {err:?}"
        );

        // Pre-existing dangling reference (written out-of-band): an unrelated
        // repair edit is not blocked by it.
        let mut broken = pipeline_store::read_binding(root, "v", "p").unwrap();
        broken.source_facets = vec!["missing".to_string()];
        pipeline_store::write_binding(root, "v", "p", &broken).unwrap();
        let repaired = update_binding_json(root, "v", "p", r#"{"intent":"repair"}"#).unwrap();
        assert_eq!(repaired.intent.as_deref(), Some("repair"));
    }

    #[test]
    fn update_binding_json_refuses_missing_record() {
        let tmp = binding_fixture();
        let root = tmp.path();
        let err = update_binding_json(root, "v", "nope", r#"{"intent":"x"}"#).unwrap_err();
        assert!(
            matches!(err, PipelineEditError::NotFound { .. }),
            "got {err:?}"
        );
    }

    /// Unknown additive keys are tolerated and `version` is engine-managed —
    /// a payload carrying either never fails, never moves the version.
    #[test]
    fn update_binding_json_ignores_unknown_keys_and_version() {
        let tmp = binding_fixture();
        let root = tmp.path();
        add_binding_json(
            root,
            "v",
            "p",
            r#"{"source_facets":["f"],"destination_mem":"v"}"#,
        )
        .unwrap();
        let after = update_binding_json(
            root,
            "v",
            "p",
            r#"{"version": 99, "future_key": { "x": 1 }, "intent": "i2"}"#,
        )
        .unwrap();
        assert_eq!(after.version, BINDING_VERSION, "version stays engine-managed");
        assert_eq!(after.intent.as_deref(), Some("i2"));
    }
}
