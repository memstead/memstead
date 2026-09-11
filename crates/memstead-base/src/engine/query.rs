//! Engine read paths — accessors and queries.
//!
//! Read-only methods on `Engine`: store / schema / mount accessors,
//! per-mem path helpers (`gitdir_for` / `worktree_for`), aggregated
//! views (`communities`, `orphans`, `stubs`, `most_connected`,
//! `missing_required_outgoing`), per-mem summaries (`health`,
//! `status`, `context`), search (`list`, `search`,
//! `search_indexes`), and the bytes-level read wrappers
//! (`list_entities`, `read_entity`, `read_provenance`). Capability and
//! cross-mem link gating live here too — they're consulted by
//! handlers before any mutation reaches the backend.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use memstead_schema::Schema;

use crate::engine_fallback_type;
use crate::entity::{Entity, EntityId};
use crate::graph::{LouvainOutput, community::detect_communities};
use crate::mem::MemRouterSnapshot;
use crate::ops::{ContextResult, Direction, NeighborInfo, SearchResult, SearchScope, WarningHint};
use crate::provenance::Provenance;
#[cfg(not(target_arch = "wasm32"))]
use crate::search_index::{MemIndex, build_all};
use crate::store::Store;
use crate::workspace::{MountCapability, MountStorage, WorkspaceSettings};

use super::{BackendFactory, Engine, EngineError, MountedBackend};

impl Engine {
    /// In-memory store populated at construction time from every
    /// mount's backend. Read-only at this point in the rebuild —
    /// mutation paths land in a later session.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Per-mem schema, keyed by mount's mem name. Each entry is the
    /// schema resolved from that mount's pin at boot, so the map holds
    /// genuinely heterogeneous schemas in a multi-schema workspace.
    pub fn schemas(&self) -> &HashMap<String, Arc<Schema>> {
        &self.schemas
    }

    /// Workspace-authored schemas loaded from
    /// `WorkspaceSettings.schemas_dir` at construction. Distinct from
    /// [`Self::schemas`] (per-mem, only schemas pinned by a mount):
    /// this slice carries every workspace-loaded schema regardless of
    /// whether a mem pins it. Used by `memstead_overview` to enumerate
    /// schemas referenced by `mem_create_rules.schemas[]` but not
    /// pinned by any mem — agents see what could be pinned without
    /// looking up the workspace.toml directly.
    pub fn workspace_schemas(&self) -> &[Arc<Schema>] {
        &self.workspace_schemas
    }

    /// Embedded built-in schemas loaded once at boot. Handlers
    /// resolving a schema pin by `<name>@<version>` (MCP's `memstead_schema`,
    /// `memstead_overview` rendering) walk mem-pinned, workspace, and
    /// built-in catalogues in order — built-ins are the catch-all when
    /// no mem or workspace dir pins the schema. Workspace schemas
    /// shadow built-ins on `(name, version)` collision; resolve from
    /// `workspace_schemas()` first.
    pub fn builtin_schemas(&self) -> &[Arc<Schema>] {
        &self.builtin_schemas
    }

    /// Classify a schema's trust origin — the single authority every read
    /// surface consults before serving a schema's instruction-prose.
    ///
    /// A schema is [`OriginClass::FirstParty`] iff it is an engine built-in
    /// **or** pinned by a writable mount in this workspace. Built-ins are
    /// compiled into the binary — unforgeable. A non-built-in schema earns
    /// first-party status only once the operator *adopts* it by writably
    /// mounting a mem that pins it: writing into a mem is the act that
    /// legitimately needs a schema's authoring prose (`system_message`,
    /// `write_rules`, …), and the mount's writable posture is set by the
    /// consumer's own config — a publisher cannot forge it.
    ///
    /// Everything else is [`OriginClass::ThirdParty`]: a schema present in
    /// the catalogue but pinned only by read-only mounts (a registry-
    /// installed read-mem or an adopted foreign folder/clone), or one the
    /// engine cannot vouch for at all. Its prose is served structural-only
    /// so a stranger's free-text never reaches a consuming agent as
    /// instructions. This classifies by the mount graph — never by scanning
    /// the schema's content, which a publisher controls — and `ThirdParty`
    /// is the safe default for any ambiguous origin.
    ///
    /// Note a read-only mount pinning a *built-in* schema (e.g. a registry
    /// mem on `default@1.0.0`) resolves to the consumer's own clean copy
    /// and stays first-party — the de-framing targets only foreign,
    /// non-built-in schemas that no writable mem has adopted.
    pub fn schema_origin(&self, schema: &Arc<Schema>) -> crate::render::OriginClass {
        use crate::render::OriginClass;
        let (name, version) = schema.id();
        // Built-in schemas are compiled in — first-party, unforgeable.
        let is_builtin = self.builtin_schemas.iter().any(|s| {
            let id = s.id();
            id.0 == name && id.1 == version
        });
        if is_builtin {
            return OriginClass::FirstParty;
        }
        // Adoption signal: some writable mount pins this exact schema, so
        // the operator authors against it here.
        let canon = format!("{name}@{version}");
        let pinned_by_writable = self.mounts().iter().any(|m| {
            m.schema.as_ref().map(|s| s.to_string()).as_deref() == Some(canon.as_str())
                && self.mem_router().is_writable(&m.mem)
        });
        if pinned_by_writable {
            OriginClass::FirstParty
        } else {
            OriginClass::ThirdParty
        }
    }

    /// Classify a mem's *data* trust origin — the authority every read
    /// surface consults before serving an entity's content (bodies,
    /// snippets, titles). A writable mount is [`OriginClass::FirstParty`]:
    /// its content is authored in this workspace. Anything else — a
    /// read-only mount (a registry-installed read-mem or an adopted
    /// foreign folder/clone) or an unknown mem — is
    /// [`OriginClass::ThirdParty`], so the consuming agent/host treats the
    /// content as quoted, untrusted data.
    ///
    /// This reads the deployment's declaration when one exists (see
    /// [`Self::declare_mem_origin`]), else the mount's already-decided
    /// writable/read-only posture (fixed at adopt/mount time) — it never
    /// scans content, and both levers are consumer-side config, so a
    /// publisher cannot forge first-party. Distinct from
    /// [`Self::schema_origin`], which governs a schema's
    /// instruction-prose: the data channel and the instruction channel
    /// are separate vectors with separate authorities.
    pub fn mem_origin_class(&self, mem: &str) -> crate::render::OriginClass {
        if let Some(declared) = self.declared_origins.get(mem) {
            return *declared;
        }
        if self.mem_router().is_writable(mem) {
            crate::render::OriginClass::FirstParty
        } else {
            crate::render::OriginClass::ThirdParty
        }
    }

    /// Declare a mem's data-trust origin as a deployment fact — the
    /// embedding process (a curated hosted read tier, an app that vouches
    /// for a bundled mem) overrides the writability inference for one mem.
    /// Composition-layer-only by design: not persisted, not reachable over
    /// MCP, never derived from mem content — the operator running the
    /// process is the only authority that can set it, so a served mem the
    /// deployment does *not* vouch for keeps reporting third-party on
    /// every surface. (Deliberately absent from the CLI: that surface
    /// operates a workspace, not a deployment; the CLI counterpart would be
    /// a workspace-config knob no use case demands yet.)
    pub fn declare_mem_origin(
        &mut self,
        mem: impl Into<String>,
        origin: crate::render::OriginClass,
    ) {
        self.declared_origins.insert(mem.into(), origin);
    }

    /// Per-file errors collected during load. Non-fatal: the engine
    /// continues with whatever did parse. Empty when every backend's
    /// content parses cleanly.
    pub fn load_errors(&self) -> &[(PathBuf, String)] {
        &self.load_errors
    }

    /// Resolve a VISIBLE mem to its folder-storage root on disk.
    ///
    /// Unknown or quarantined names refuse `UNKNOWN_MEM` (the same
    /// visibility gate `search` and the conflicts door apply); a
    /// visible mount whose storage is not folder-backed returns
    /// `Ok(None)` so callers branch on backend applicability without
    /// inventing a typed error. Read-only mounts resolve too — this is
    /// a read accessor, not a write gate. Generic by design: any
    /// consumer that needs a folder mem's disk root (per-mem changelog
    /// readers, export tooling, future doors) gets the same answer.
    pub fn folder_mem_root(&self, mem: &str) -> Result<Option<PathBuf>, EngineError> {
        let mount = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(mem))?;
        if self.quarantine_reason(mem).is_some() {
            return Err(self.unknown_mem_error(mem));
        }
        match &mount.mount.storage {
            crate::workspace::MountStorage::Folder { path } => Ok(Some(path.clone())),
            _ => Ok(None),
        }
    }

    /// Workspace-level operator policy (mem create/delete rules,
    /// cross-mem links). Defaults to empty; populated via
    /// [`Engine::set_settings`] after construction. Surfaced for MCP
    /// handlers (`memstead_health { include_config: true }`,
    /// `memstead_overview`'s lifecycle-namespaces section) and other
    /// consumers that need to read workspace policy.
    pub fn settings(&self) -> &WorkspaceSettings {
        &self.settings
    }

    /// The pipeline configs — the v2 single-record binding store — loaded
    /// from the workspace at boot: the read-only queryable surface the
    /// loader exposes. Empty for engines not booted from a workspace root,
    /// or for a workspace that declares no pipelines. The ingest skill
    /// and future MCP tools consume this structured form
    /// rather than re-reading the JSON folders.
    pub fn pipeline_configs(&self) -> &crate::pipeline_store::BindingConfigs {
        &self.pipeline_configs
    }

    /// The pipeline configs serialized as a JSON string — the read
    /// counterpart of the `add_projection_json` edit entry point.
    /// Serialization-boundary callers (where serde does not live)
    /// get the store in one call and deserialize on their side.
    ///
    /// Shape: `{ "bindings": [{ mem, name, config }] }` — the v2
    /// single-record store (`config` carries the whole binding: inline
    /// `sources`, `operations`, everything). The `mediums` / `facets` /
    /// `ingests` keys are **gone** with their record kinds. This reads the
    /// live binding store fresh (like the brief path) rather than the
    /// in-memory snapshot, so an edit shows back immediately. A missing
    /// root or a legacy/unreadable store yields the fallback empty object.
    pub fn pipeline_configs_json(&self) -> String {
        let empty = || "{\"bindings\":[]}".to_string();
        let Some(root) = self.workspace_root() else {
            return empty();
        };
        match crate::pipeline_store::load_pipeline_configs(root) {
            Ok(configs) => serde_json::to_string(&configs).unwrap_or_else(|_| empty()),
            Err(_) => empty(),
        }
    }

    /// Overwrite the in-memory pipeline configs. The workspace-root boot
    /// paths call this after [`crate::pipeline_store::load_pipeline_configs`];
    /// exposed so the full boot helper (a separate crate) can populate the
    /// same surface.
    pub fn set_pipeline_configs(&mut self, configs: crate::pipeline_store::BindingConfigs) {
        self.pipeline_configs = configs;
    }

    /// Build a [`WarningHint::NoteMissing`] when the workspace has
    /// `[mutations].require_notes = true` and the caller omitted (or
    /// passed a blank/whitespace-only) `note`; `None` otherwise.
    ///
    /// This is the single enforcement point for the `require_notes`
    /// provenance nudge. Every mutation that accepts a `note` calls it
    /// on its commit-landing path and pushes the result onto the
    /// outcome's `warnings`, so both the CLI and the MCP transports
    /// inherit identical behaviour from the engine response rather than
    /// each re-deriving the policy at its own boundary (the drift that
    /// left the policy decorative on the CLI). `tool` becomes the
    /// warning's `details.tool` — callers pass the engine-level verb
    /// (`create_entity`, `update_entity`, `relate_entity`,
    /// `delete_entity`, `rename_entity`, `create_mem`,
    /// `delete_mem`), matching the commit `Tool:` provenance trailer.
    /// The mutation still commits — the policy nudges, it never blocks.
    pub fn note_missing_warning(&self, tool: &str, note: Option<&str>) -> Option<WarningHint> {
        if !self.settings.mutations.require_notes.unwrap_or(false) {
            return None;
        }
        let has_note = note.map(|n| !n.trim().is_empty()).unwrap_or(false);
        if has_note {
            return None;
        }
        Some(WarningHint::NoteMissing {
            tool: tool.to_string(),
        })
    }

    /// Backend factory currently installed on this engine. Returned by
    /// value because [`BackendFactory`] is a function pointer (`Copy`).
    /// Used by [`crate::mem_management::create_mem`] to materialise
    /// the backend for a freshly-registered mount; consumers that need
    /// to instantiate a backend ad-hoc can call this directly.
    pub fn backend_factory(&self) -> BackendFactory {
        self.backend_factory
    }

    /// Git-branch ops bundle currently installed on this engine.
    /// `None` on engines without the git-branch crate, which see no mem-repo
    /// mounts. Returned by value because [`super::GitBranchOps`] is
    /// `Copy`. `create_mem` reaches for
    /// the bundle to drive `prune_residue` against an unmounted
    /// gitdir when the `ForceOverwrite` recovery action is selected.
    pub fn git_branch_ops(&self) -> Option<super::GitBranchOps> {
        self.git_branch_ops
    }

    /// Whether a cross-mem edge from `from_mem` to `to_mem` is
    /// permitted under the current [`crate::WorkspaceSettings`]
    /// cross-mem link policy.
    ///
    /// Resolution rules (matches full's `mem_router` semantics):
    /// 1. Same-mem edge (`from_mem == to_mem`) → always
    ///    allowed; the policy gates *cross*-mem edges only.
    /// 2. Explicit `cross_mem_links[from_mem]`:
    ///    - `"*"` (wildcard) → allowed regardless of target.
    ///    - `["a", ...]` (allowlist) → allowed iff `to_mem` is in
    ///      the list.
    /// 3. Per-create-rule `default_cross_links` synthesis — if
    ///    rule (1) didn't grant permission and `from_mem` matches
    ///    a `[[mem_management.create]]` rule whose
    ///    `default_cross_links` is set, the synthesised value
    ///    contributes:
    ///    - `"*"` → allowed regardless of target.
    ///    - `["a", ...]` → allowed iff `to_mem` is in the list.
    /// 4. Otherwise → denied (default-deny posture).
    ///
    /// The synthesis layer compiles a [`crate::mem_management::CreateRuleSet`]
    /// lazily on first call and caches it; [`Self::set_settings`]
    /// invalidates the cache. Compilation failure (malformed glob
    /// in a rule) logs a warning and the synthesis layer is silently
    /// skipped — the resolver still returns `true` from explicit
    /// policy alone, so a half-broken config doesn't lock out edges
    /// the operator did intend to allow. Operators who want hard
    /// validation pre-compile via
    /// [`crate::mem_management::CreateRuleSet::new`] before
    /// calling [`Self::set_settings`].
    ///
    /// The MCP `memstead_relate` handler's cross-mem gate consumes
    /// this method directly.
    pub fn cross_mem_link_allowed(&self, from_mem: &str, to_mem: &str) -> bool {
        use memstead_schema::workspace_config::CrossLinkValue;
        if from_mem == to_mem {
            return true;
        }

        // Step 1: explicit cross_mem_links policy.
        if let Some(value) = self.settings.cross_mem_links.get(from_mem) {
            match value {
                CrossLinkValue::Wildcard => return true,
                CrossLinkValue::List(targets) => {
                    if targets.iter().any(|t| t == to_mem) {
                        return true;
                    }
                    // Fall through to synthesis check — a List that
                    // doesn't include the target may still allow it
                    // via per-rule default_cross_links union.
                }
            }
        }

        // Step 2: per-create-rule default_cross_links synthesis.
        let rule_set = self.create_rule_set_memo.get_or_init(|| {
            crate::mem_management::CreateRuleSet::new(
                self.settings.mem_create_rules.clone(),
            )
            .unwrap_or_else(|err| {
                tracing::warn!(
                    error = %err,
                    "cross_mem_link_allowed: failed to compile mem_create_rules — synthesis disabled (resolver falls back to explicit-policy-only)"
                );
                crate::mem_management::CreateRuleSet::default()
            })
        });

        // Compose the same `<mem_path>/<name>` candidate the create-rule
        // composer matched against. The rule globs are keyed on the composed
        // lifecycle path (e.g. `memstead/project`, compiled with
        // `literal_separator`), not the bare leaf name — matching
        // `from_mem` alone silently misses, so synthesis denied a link
        // that `memstead_overview` rendered as rule-granted (the
        // leaf-vs-composed-path divergence). Flat-layout mems (no
        // hierarchical path) keep the bare leaf, matching their bare rule.
        let candidate = match self.mount(from_mem).and_then(|m| m.mem_path()) {
            Some(path) => format!("{path}/{from_mem}"),
            None => from_mem.to_string(),
        };
        if let Some(matched) = rule_set.first_match(std::path::Path::new(&candidate))
            && let Some(synth) = matched.default_cross_links.as_ref()
        {
            return match synth {
                CrossLinkValue::Wildcard => true,
                CrossLinkValue::List(targets) => targets.iter().any(|t| t == to_mem),
            };
        }

        false
    }

    pub(super) fn find_mount(&self, mem: &str) -> Result<&MountedBackend, EngineError> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(mem))
    }
}

// -------------------------------------------------------------------------
// Cut by concern. Each child opens its own `impl Engine` block, so every
// method keeps its name, signature and visibility and no consumer moves.
// -------------------------------------------------------------------------

mod anchors;
mod health;
mod mounts;
mod reads;

pub use anchors::*;

#[cfg(test)]
mod tests;
