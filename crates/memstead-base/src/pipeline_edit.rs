//! Edit operations over the v2 single-record pipeline store.
//!
//! Sits above the dumb file ops in [`crate::pipeline_store`]: each function
//! loads the current store, enforces identity rules (no clobber, no silent
//! overwrite), validates the candidate record in-place, then writes. The
//! engine's in-memory `pipeline_configs` cache is refreshed by the `Engine`
//! wrapper methods that call these — not here — so these functions stay pure
//! disk ops that unit-test with a bare `TempDir`.
//!
//! One record kind, four verbs: a binding is created / patched / deleted /
//! renamed under its `(mem, name)` file identity. The cross-record
//! referential-integrity machinery of the three-file era (dangling facet /
//! medium refs, delete-blocked-by-referrer, rename repointing) is gone with
//! the references — **in-record source validation**
//! ([`crate::binding::validate_binding`]) covers what remains: source name
//! rules plus the medium-capability matrix.
//!
//! The JSON entry points ([`add_binding_json`] / [`update_binding_json`])
//! accept a [`BindingPatch`] over the full author-editable record —
//! `sources`, operations block, `deny_paths`, `coverage_semantics`, `rules`
//! (clearable via explicit `null`), `prune` — with `version` engine-managed.
//! A field absent from the payload is preserved from the stored record (the
//! tail-preservation contract): edits merge over the stored record, never
//! rebuild it from form fields.

use std::path::Path;

use serde::Deserialize;

use crate::binding::{
    BINDING_VERSION, Binding, BuildMode, BuildOperation, CapabilityError, CoverageSemantics,
    Operations, PruneConfig, validate_binding,
};
use crate::engine::Engine;
use crate::pipeline::{IngestTrigger, Source};
use crate::pipeline_store::{self, BindingConfigs};
use crate::workspace_store::StoreError;

/// Failure modes of a pipeline edit. Distinct from the entity-centric
/// [`crate::engine::EngineError`] — these describe binding-store edits.
/// `key` is the display identity: `"<mem>/<name>"`.
#[derive(Debug, thiserror::Error)]
pub enum PipelineEditError {
    /// The engine was not booted from a workspace root, so there is no
    /// binding store to edit (e.g. an engine built from a bare mount
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
    /// A rename target `(mem, new)` already holds a record.
    #[error("rename target {primitive} '{key}' already exists")]
    RenameTargetExists {
        primitive: &'static str,
        key: String,
    },
    /// A JSON-string edit entry point received a payload that did not
    /// deserialize into the target shape.
    #[error("invalid {primitive} JSON: {message}")]
    InvalidJson {
        primitive: &'static str,
        message: String,
    },
    /// A binding edit was refused by in-record validation — a malformed
    /// source (empty / duplicate name) or a medium-capability violation
    /// (e.g. declaring `sync` over a medium with no change signal). Carries
    /// only the refusals the edit would *introduce*: refusals the stored
    /// record already produces never block an unrelated edit. Each refusal
    /// message includes its remedy.
    #[error(
        "binding '{key}' edit refused by validation: {}",
        format_refusals(refusals)
    )]
    Capability {
        key: String,
        refusals: Vec<CapabilityError>,
    },
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

fn binding_exists(c: &BindingConfigs, mem: &str, name: &str) -> bool {
    c.bindings.iter().any(|r| r.mem == mem && r.name == name)
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
/// the stored record — the tail-preservation guarantee extends to every
/// field. A field present is applied. The natively-optional record fields
/// (`intent`, `rules`, `prune`) additionally distinguish explicit `null`
/// (clear) from absence (preserve). `sources` and `operations` are replaced
/// as whole blocks when present: each is the unit of its authoring, so an
/// entry absent from a supplied block is removed (for operations that is
/// legal — an absent mutating op refuses at run time with its enable
/// remedy).
///
/// `version` is engine-managed and never author input: a `version` key in
/// the payload is ignored, like every unknown key (the format grows
/// additively — tolerance is deliberate).
#[derive(Debug, Default, Deserialize)]
pub struct BindingPatch {
    /// Set / clear (`null`) / preserve (absent) the binding's intent prose.
    #[serde(default, deserialize_with = "patch_field")]
    pub intent: Option<Option<String>>,
    /// Replace the inline source list when present.
    #[serde(default)]
    pub sources: Option<Vec<Source>>,
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
    fn apply(self, binding: &mut Binding) {
        if let Some(v) = self.intent {
            binding.intent = v;
        }
        if let Some(v) = self.sources {
            binding.sources = v;
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

/// The default scaffold a fresh binding is patched onto: version pinned, a
/// default `build` block (discovery / loop / batch 20), sync and verify
/// absent, everything else empty — the caller's patch overlays it.
fn default_binding_scaffold() -> Binding {
    Binding {
        version: BINDING_VERSION,
        intent: None,
        sources: Vec::new(),
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

/// Create a binding from a JSON [`BindingPatch`] applied to the default
/// scaffold. Refuses when `(mem, name)` already holds a record
/// ([`PipelineEditError::AlreadyExists`]), when the patch leaves
/// `destination_mem` empty ([`PipelineEditError::InvalidJson`]), or when
/// the candidate fails in-record validation
/// ([`PipelineEditError::Capability`] — every refusal blocks; a fresh
/// record has no pre-existing state to grandfather). Returns the written
/// record.
pub fn add_binding_json(
    root: &Path,
    mem: &str,
    name: &str,
    patch_json: &str,
) -> Result<Binding, PipelineEditError> {
    let configs = pipeline_store::load_pipeline_configs(root)?;
    if binding_exists(&configs, mem, name) {
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
    if let Err(refusals) = validate_binding(&binding) {
        return Err(PipelineEditError::Capability {
            key: key(mem, name),
            refusals,
        });
    }
    pipeline_store::write_binding(root, mem, name, &binding)?;
    Ok(binding)
}

/// Patch an existing binding from a JSON [`BindingPatch`] — the update path
/// over the full author-editable record (`sources`, operations,
/// `deny_paths`, `coverage_semantics`, `rules` including clearing,
/// `prune`). Absent fields are preserved (the tail-preservation contract);
/// explicit `null` clears `intent` / `rules` / `prune`; a present `sources`
/// or `operations` block replaces that whole block; `version` stays
/// engine-managed.
///
/// Refuses when the record does not exist
/// ([`PipelineEditError::NotFound`]) or when the patch introduces a
/// validation refusal the stored record did not already carry
/// ([`PipelineEditError::Capability`] — pre-existing refusals never block
/// an unrelated edit). Nothing is written on refusal. Returns the written
/// record.
pub fn update_binding_json(
    root: &Path,
    mem: &str,
    name: &str,
    patch_json: &str,
) -> Result<Binding, PipelineEditError> {
    let configs = pipeline_store::load_pipeline_configs(root)?;
    if !binding_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "projection",
            key: key(mem, name),
        });
    }
    let patch: BindingPatch = parse_json(patch_json, "projection")?;
    let existing = pipeline_store::read_binding(root, mem, name)?;
    let mut patched = existing.clone();
    patch.apply(&mut patched);
    if let Err(refusals) = validate_binding(&patched) {
        let before = validate_binding(&existing).err().unwrap_or_default();
        let introduced: Vec<CapabilityError> = refusals
            .into_iter()
            .filter(|r| !before.contains(r))
            .collect();
        if !introduced.is_empty() {
            return Err(PipelineEditError::Capability {
                key: key(mem, name),
                refusals: introduced,
            });
        }
    }
    pipeline_store::write_binding(root, mem, name, &patched)?;
    Ok(patched)
}

/// Delete a binding. Refuses if `(mem, name)` does not exist. Nothing
/// references a binding record, so there is no referential gate — the
/// binding's own state files (advance / findings / watermarks) simply stop
/// being consulted.
pub fn delete_binding(root: &Path, mem: &str, name: &str) -> Result<(), PipelineEditError> {
    let configs = pipeline_store::load_pipeline_configs(root)?;
    if !binding_exists(&configs, mem, name) {
        return Err(PipelineEditError::NotFound {
            primitive: "projection",
            key: key(mem, name),
        });
    }
    pipeline_store::delete_projection(root, mem, name)?;
    Ok(())
}

/// Rename a binding within its mem (`old` → `new`, same `<mem>` tier). A
/// binding has no embedded name, so a file move is its whole rename.
/// Refuses a missing source or an existing target. No-op when `old == new`.
pub fn rename_binding(
    root: &Path,
    mem: &str,
    old: &str,
    new: &str,
) -> Result<(), PipelineEditError> {
    if old == new {
        return Ok(());
    }
    let configs = pipeline_store::load_pipeline_configs(root)?;
    if !binding_exists(&configs, mem, old) {
        return Err(PipelineEditError::NotFound {
            primitive: "projection",
            key: key(mem, old),
        });
    }
    if binding_exists(&configs, mem, new) {
        return Err(PipelineEditError::RenameTargetExists {
            primitive: "projection",
            key: key(mem, new),
        });
    }
    pipeline_store::rename_projection(root, mem, old, new)?;
    Ok(())
}

// --- Engine surface --------------------------------------------------------
//
// Thin wrappers that route an edit through the free functions above (disk +
// in-record validation) and then refresh the engine's in-memory
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
        self.set_pipeline_configs(pipeline_store::load_pipeline_configs(root)?);
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

    /// Create a binding from a JSON [`BindingPatch`] applied to the default
    /// scaffold: the caller may supply any author-editable field, including
    /// the inline `sources` and a full `operations` block; an absent block
    /// scaffolds the default `build` (discovery / loop / batch 20). See
    /// [`add_binding_json`] for the refusals (duplicate, missing
    /// `destination_mem`, in-record validation).
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

    /// Patch a binding from a JSON [`BindingPatch`] — absent fields are
    /// preserved (tail preservation, extended to every field); explicit
    /// `null` clears `intent` / `rules` / `prune`; a present `sources` or
    /// `operations` block replaces that whole block; `version` stays
    /// engine-managed. See [`update_binding_json`] for the refusals
    /// (not-found, in-record validation).
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

    /// Delete a binding and refresh the snapshot. See [`delete_binding`].
    pub fn delete_projection(
        &mut self,
        mem: &str,
        name: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        delete_binding(&root, mem, name)?;
        self.pipeline_provenance(
            mem,
            "projections",
            &[(name.to_string(), None)],
            note,
            "delete",
        )?;
        self.refresh_pipeline_configs(&root)
    }

    /// Rename a binding and refresh the snapshot. See [`rename_binding`].
    pub fn rename_projection(
        &mut self,
        mem: &str,
        old: &str,
        new: &str,
        note: Option<&str>,
    ) -> Result<(), PipelineEditError> {
        let root = self.pipeline_edit_root()?;
        rename_binding(&root, mem, old, new)?;
        // Mirror the rename as remove-old + upsert-new in one commit.
        // The in-memory snapshot still holds the record under its old
        // name here (refresh runs after), and a rename changes the
        // name only — the config bytes are those of the old record.
        let bytes = self
            .pipeline_configs()
            .bindings
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

    /// Shared provenance + refresh tail for the binding edit path: the disk
    /// write has already landed in the free function; record the edit's
    /// mirror commit and refresh the in-memory snapshot.
    fn record_binding_edit(
        &mut self,
        mem: &str,
        name: &str,
        binding: &Binding,
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

/// Deserialize a pipeline shape from JSON, mapping a parse failure to a
/// typed [`PipelineEditError::InvalidJson`] naming the shape.
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
    use crate::binding::{PruneGuarantee, SyncOperation};
    use tempfile::TempDir;

    /// The base payload with one inline codebase source.
    const BASE_PAYLOAD: &str = r#"{
        "intent": "i",
        "sources": [{
            "name": "f",
            "type": "codebase",
            "pointer": "../src",
            "scope": [{ "path": "**/*.rs", "mode": "allow" }]
        }],
        "reference_mems": [],
        "destination_mem": "v",
        "rules": { "routing": "r" }
    }"#;

    /// The full record used by the update tests: build+sync+verify over the
    /// codebase source, deny_paths, curated coverage, rules, prune.
    fn full_binding_payload() -> &'static str {
        r#"{
          "intent": "i",
          "sources": [{
              "name": "f",
              "type": "codebase",
              "pointer": "../src",
              "scope": [{ "path": "**/*.rs", "mode": "allow" }]
          }],
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

    /// A payload creates a binding scaffolded with the default build block;
    /// the inline source is written as given.
    #[test]
    fn add_binding_json_scaffolds_default_build() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let b = add_binding_json(root, "v", "p", BASE_PAYLOAD).unwrap();
        assert_eq!(b.version, BINDING_VERSION);
        let build = b.operations.build.as_ref().unwrap();
        assert_eq!(build.mode, BuildMode::Discovery);
        assert_eq!(build.batch_size, 20);
        assert!(b.operations.sync.is_none() && b.operations.verify.is_none());
        assert_eq!(b.rules, Some(serde_json::json!({ "routing": "r" })));
        assert_eq!(b.sources.len(), 1);
        assert_eq!(b.sources[0].name, "f");
        assert_eq!(pipeline_store::read_binding(root, "v", "p").unwrap(), b);
    }

    /// A payload declaring the full record — operations block, deny_paths,
    /// coverage, prune — is written as given (the scaffold is only a fallback).
    #[test]
    fn add_binding_json_accepts_the_full_record() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let b = add_binding_json(root, "v", "p", full_binding_payload()).unwrap();
        assert_eq!(
            b.operations.build.as_ref().unwrap().mode,
            BuildMode::Discovery
        );
        assert_eq!(b.operations.sync.as_ref().unwrap().batch_size, 20);
        assert!(b.operations.verify.is_some());
        assert_eq!(b.deny_paths, vec!["dev/**"]);
        assert_eq!(b.coverage_semantics, CoverageSemantics::Curated);
        assert_eq!(
            b.prune.as_ref().unwrap().guarantee,
            PruneGuarantee::NeverClobber
        );
    }

    #[test]
    fn add_binding_json_refuses_duplicate() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_binding_json(root, "v", "p", BASE_PAYLOAD).unwrap();
        let err = add_binding_json(root, "v", "p", BASE_PAYLOAD).unwrap_err();
        assert!(
            matches!(err, PipelineEditError::AlreadyExists { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn add_binding_json_requires_destination_mem() {
        let tmp = TempDir::new().unwrap();
        let err = add_binding_json(tmp.path(), "v", "p", r#"{"intent":"i"}"#).unwrap_err();
        match err {
            PipelineEditError::InvalidJson { message, .. } => {
                assert!(message.contains("destination_mem"), "got: {message}")
            }
            other => panic!("expected InvalidJson, got {other:?}"),
        }
    }

    /// REFUSAL (plan criterion 3) — a malformed source (duplicate name)
    /// blocks a create with a typed validation error; nothing lands on disk.
    #[test]
    fn add_binding_json_refuses_duplicate_source_names() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let err = add_binding_json(
            root,
            "v",
            "p",
            r#"{
              "sources": [
                { "name": "dup", "type": "codebase", "pointer": "../a" },
                { "name": "dup", "type": "codebase", "pointer": "../b" }
              ],
              "destination_mem": "v"
            }"#,
        )
        .unwrap_err();
        match err {
            PipelineEditError::Capability { refusals, .. } => {
                assert!(
                    refusals.iter().any(|r| matches!(
                        r,
                        CapabilityError::DuplicateSourceName { name } if name == "dup"
                    )),
                    "expected DuplicateSourceName, got {refusals:?}"
                );
            }
            other => panic!("expected Capability, got {other:?}"),
        }
        assert!(!root.join(".memstead/projections/v/p.json").exists());
    }

    /// The capability matrix at the edit seam: declaring sync over a web
    /// (change-signal-less) source refuses at add time with the matrix's
    /// typed message.
    #[test]
    fn add_binding_json_refuses_capability_violation() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let err = add_binding_json(
            root,
            "v",
            "p",
            r#"{
              "sources": [{ "name": "wf", "type": "web", "pointer": "https://example.com" }],
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

    /// Patch semantics: a single-field patch preserves every sibling field —
    /// the tail-preservation property, extended to the full record.
    #[test]
    fn update_binding_json_patch_preserves_untouched_fields() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let before = add_binding_json(root, "v", "p", full_binding_payload()).unwrap();
        let after = update_binding_json(root, "v", "p", r#"{"intent":"new"}"#).unwrap();
        assert_eq!(after.intent.as_deref(), Some("new"));
        assert_eq!(after.version, before.version);
        assert_eq!(after.sources, before.sources);
        assert_eq!(after.reference_mems, before.reference_mems);
        assert_eq!(after.destination_mem, before.destination_mem);
        assert_eq!(after.deny_paths, before.deny_paths);
        assert_eq!(after.coverage_semantics, before.coverage_semantics);
        assert_eq!(after.rules, before.rules);
        assert_eq!(after.prune, before.prune);
        assert_eq!(after.operations, before.operations);
    }

    /// Explicit `null` clears the natively-optional fields; absence preserves
    /// them.
    #[test]
    fn update_binding_json_null_clears_and_absence_preserves() {
        let tmp = TempDir::new().unwrap();
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
    /// A present `sources` block likewise replaces the whole source list.
    #[test]
    fn update_binding_json_replaces_whole_blocks() {
        let tmp = TempDir::new().unwrap();
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
        assert!(
            after.operations.sync.is_none(),
            "sync removed with the block"
        );
        assert!(
            after.operations.verify.is_none(),
            "verify removed with the block"
        );
        assert_eq!(after.rules, Some(serde_json::json!({ "routing": "r" })));

        let after = update_binding_json(
            root,
            "v",
            "p",
            r#"{"sources": [{ "name": "g", "type": "filesystem", "pointer": "../docs" }]}"#,
        )
        .unwrap();
        assert_eq!(after.sources.len(), 1);
        assert_eq!(after.sources[0].name, "g");
    }

    /// The update seam refuses an *introduced* capability violation, and the
    /// stored record stays byte-identical.
    #[test]
    fn update_binding_json_refuses_introduced_capability_violation() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let before = add_binding_json(
            root,
            "v",
            "p",
            r#"{
              "sources": [{ "name": "wf", "type": "web", "pointer": "https://example.com" }],
              "destination_mem": "v"
            }"#,
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
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Bypass validation: a sync-over-web binding already on disk (the
        // store layer is dumb on purpose).
        let broken = Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![Source {
                name: "wf".to_string(),
                medium_type: crate::pipeline::MediumType::Web,
                pointer: "https://example.com".to_string(),
                change_detection: None,
                scope: vec![],
                engagement: None,
                preparation: None,
            }],
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
                verify: None,
            },
        };
        pipeline_store::write_binding(root, "v", "p", &broken).unwrap();

        let after = update_binding_json(root, "v", "p", r#"{"intent":"fixed"}"#).unwrap();
        assert_eq!(after.intent.as_deref(), Some("fixed"));
        assert!(
            after.operations.sync.is_some(),
            "pre-existing sync survives"
        );

        // But a patch introducing a NEW refusal (duplicate source names) blocks.
        let err = update_binding_json(
            root,
            "v",
            "p",
            r#"{"sources": [
                { "name": "dup", "type": "web", "pointer": "https://a" },
                { "name": "dup", "type": "web", "pointer": "https://b" }
            ]}"#,
        )
        .unwrap_err();
        assert!(
            matches!(err, PipelineEditError::Capability { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn update_binding_json_refuses_missing_record() {
        let tmp = TempDir::new().unwrap();
        let err = update_binding_json(tmp.path(), "v", "p", r#"{"intent":"x"}"#).unwrap_err();
        assert!(
            matches!(err, PipelineEditError::NotFound { .. }),
            "got {err:?}"
        );
    }

    /// Unknown additive keys are tolerated and `version` is engine-managed —
    /// a payload carrying either never fails, never moves the version.
    #[test]
    fn update_binding_json_ignores_unknown_keys_and_version() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_binding_json(root, "v", "p", BASE_PAYLOAD).unwrap();
        let after = update_binding_json(
            root,
            "v",
            "p",
            r#"{"version": 99, "future_key": { "x": 1 }, "intent": "i2"}"#,
        )
        .unwrap();
        assert_eq!(
            after.version, BINDING_VERSION,
            "version stays engine-managed"
        );
        assert_eq!(after.intent.as_deref(), Some("i2"));
    }

    #[test]
    fn delete_binding_removes_the_record() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_binding_json(root, "v", "p", BASE_PAYLOAD).unwrap();
        delete_binding(root, "v", "p").unwrap();
        assert!(!root.join(".memstead/projections/v/p.json").exists());
        let err = delete_binding(root, "v", "p").unwrap_err();
        assert!(
            matches!(err, PipelineEditError::NotFound { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn rename_binding_moves_the_record() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let created = add_binding_json(root, "v", "old", BASE_PAYLOAD).unwrap();
        rename_binding(root, "v", "old", "new").unwrap();
        assert!(!root.join(".memstead/projections/v/old.json").exists());
        assert_eq!(
            pipeline_store::read_binding(root, "v", "new").unwrap(),
            created
        );
    }

    #[test]
    fn rename_binding_refuses_existing_target_and_missing_source() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        add_binding_json(root, "v", "a", BASE_PAYLOAD).unwrap();
        add_binding_json(root, "v", "b", BASE_PAYLOAD).unwrap();
        let err = rename_binding(root, "v", "a", "b").unwrap_err();
        assert!(
            matches!(err, PipelineEditError::RenameTargetExists { .. }),
            "got {err:?}"
        );
        let err = rename_binding(root, "v", "missing", "c").unwrap_err();
        assert!(
            matches!(err, PipelineEditError::NotFound { .. }),
            "got {err:?}"
        );
        // No-op rename is fine.
        rename_binding(root, "v", "a", "a").unwrap();
    }

    /// REFUSAL (plan criterion 1) — editing an unmigrated (pre-v2) store
    /// surfaces the loader's migrate-naming refusal — the edit layer never
    /// writes over a legacy store.
    #[test]
    fn editing_a_pre_v2_store_refuses_with_migrate_pointer() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = root.join(".memstead/projections/v");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("p.json"),
            br#"{"version": 1, "source_facets": ["f"], "destination_mem": "v", "operations": {}}"#,
        )
        .unwrap();
        let err = add_binding_json(root, "v", "q", BASE_PAYLOAD).unwrap_err();
        match err {
            PipelineEditError::Store(StoreError::LegacyProjectionStore { .. }) => {}
            other => panic!("expected LegacyProjectionStore, got {other:?}"),
        }
    }
}
