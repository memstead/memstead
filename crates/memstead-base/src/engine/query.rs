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
    /// every surface. (Deliberately absent from UniFFI/CLI: those surfaces
    /// operate a workspace, not a deployment; the CLI counterpart would be
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

    /// Workspace-level operator policy (mem create/delete rules,
    /// cross-mem links). Defaults to empty; populated via
    /// [`Engine::set_settings`] after construction. Surfaced for MCP
    /// handlers (`memstead_health { include_config: true }`,
    /// `memstead_overview`'s lifecycle-namespaces section) and other
    /// consumers that need to read workspace policy.
    pub fn settings(&self) -> &WorkspaceSettings {
        &self.settings
    }

    /// The pipeline configs (Medium / Facet / Projection / Ingest) loaded
    /// from the workspace store at boot — the read-only queryable surface
    /// the loader exposes. Empty for engines not booted from a workspace
    /// root, or for a workspace that declares no pipelines. The ingest
    /// skill, future MCP tools, and the macOS app consume this structured
    /// form rather than re-reading the JSON folders.
    pub fn pipeline_configs(&self) -> &crate::pipeline_store::PipelineConfigs {
        &self.pipeline_configs
    }

    /// The pipeline configs serialized as a JSON string — the read
    /// counterpart of the `add_*_json` edit entry points. Serialization-
    /// boundary callers (UniFFI, where serde does not live) get the store
    /// in one call and deserialize on their side.
    ///
    /// Shape (D14): `{ "mediums": [{ mem, name, config }], "facets": [...],
    /// "bindings": [{ mem, name, config }] }` — the version-gated v1 binding
    /// shape (`config` carries the binding's `operations` block). The `ingests`
    /// key is **gone**; operations are attributes of the binding, not a peer
    /// record. This reads the live binding store fresh (like the brief path)
    /// rather than the legacy in-memory snapshot, so a projection edit that
    /// preserves its operations shows them back immediately. A missing root or
    /// a legacy/unreadable store yields the fallback empty object.
    pub fn pipeline_configs_json(&self) -> String {
        let empty = || "{\"mediums\":[],\"facets\":[],\"bindings\":[]}".to_string();
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
    pub fn set_pipeline_configs(&mut self, configs: crate::pipeline_store::PipelineConfigs) {
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
    /// `None` on lean-flavor engines that don't see mem-repo
    /// mounts. Returned by value because [`super::GitBranchOps`] is
    /// `Copy`. `create_mem` reaches for
    /// the bundle to drive `prune_residue` against an unmounted
    /// gitdir when the `ForceOverwrite` recovery action is selected.
    pub fn git_branch_ops(&self) -> Option<super::GitBranchOps> {
        self.git_branch_ops
    }

    /// Convenience: look up a parsed entity by id. Returns `None` for
    /// unknown ids, including stub entries created for unresolved
    /// inline-link targets — callers that want to distinguish real
    /// from stub branch on `Entity::stub`.
    pub fn get_entity(&self, id: &EntityId) -> Option<&Entity> {
        self.store.get(id)
    }

    /// The stored provenance anchors for `id`, read from its mem's
    /// anchors sidecar. Empty for an entity with none, an unknown mem, or
    /// a backend that does not persist anchors (a pre-anchor archive / any
    /// sealed read-only mount). Additive read surface (E3a): the
    /// resolution *model* lives in [`crate::anchor`]
    /// ([`crate::anchor::resolve_anchor`] / [`crate::anchor::compose_entity_anchors`]);
    /// the live per-anchor *state* (which requires observing the source
    /// artifacts through the medium/preparation pipeline) is E3b's concern.
    pub fn entity_anchors(&self, id: &EntityId) -> Vec<crate::anchor::Anchor> {
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == id.mem()) else {
            return Vec::new();
        };
        let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
            return Vec::new();
        };
        match crate::anchor::AnchorSidecar::from_bytes(&bytes) {
            Ok(sc) => sc.get(id.as_ref()).to_vec(),
            Err(_) => Vec::new(),
        }
    }

    /// The stored anchors for `id`, each paired with its **live** resolution
    /// state when the engine could observe the source artifact this pass.
    ///
    /// Additive over [`Self::entity_anchors`]: the durable data is unchanged;
    /// `state` is the [`crate::anchor::resolve_anchor`] outcome against an
    /// observation the engine produces here. It is produced **only** for a
    /// `path`-namespace, single-medium mem (codebase / filesystem) whose
    /// medium root resolves from the workspace — the engine observes
    /// working-tree existence at the current HEAD:
    ///
    /// - artifact absent ⇒ [`AnchorState::Orphaned`](crate::anchor::AnchorState::Orphaned);
    /// - artifact present, non-hash class (`authored` / `informed-by`) ⇒
    ///   [`Resolves`](crate::anchor::AnchorState::Resolves);
    /// - artifact present, hash-bearing class (`anchored` / `derived`) ⇒ the
    ///   prepared-content hash comparison decides:
    ///   [`Resolves`](crate::anchor::AnchorState::Resolves) on a match,
    ///   [`Drifted`](crate::anchor::AnchorState::Drifted) on a stable-medium
    ///   mismatch, [`Recheck`](crate::anchor::AnchorState::Recheck) on an
    ///   unstable medium or when a hash is unavailable on either side (a
    ///   hash-less anchor, a `tree` grain, an unreadable artifact).
    ///
    /// `state` is `None` (unobserved — never a fabricated state) when the mem
    /// has no single path-medium, no workspace root, or the grain/namespace is
    /// not a filesystem path. Non-`path` mediums / commit-pinned reads stay
    /// deferred (E3b's remaining leg).
    pub fn entity_anchors_resolved(&self, id: &EntityId) -> Vec<ResolvedAnchor> {
        let anchors = self.entity_anchors(id);
        let root = self.single_path_medium_root(id.mem());
        anchors
            .into_iter()
            .map(|anchor| {
                let observed = root
                    .as_deref()
                    .and_then(|r| observe_path_anchor(r, &anchor));
                let (state, observed_hash) = match observed {
                    Some((state, hash)) => (Some(state), hash),
                    None => (None, None),
                };
                ResolvedAnchor {
                    anchor,
                    state,
                    observed_hash,
                }
            })
            .collect()
    }

    /// The observation root for `mem`'s single `path`-namespace medium
    /// (codebase / filesystem), or `None` when the mem has zero / several
    /// mediums, no workspace root, or its lone medium is not path-shaped
    /// (`path+commit` / `entity` / `url` — those need commit-pinned or
    /// non-filesystem observation, E3b). The root is the **workspace root**:
    /// anchor artifact ids are workspace-relative (pointer-prefixed) — the
    /// same dialect enumeration, deny_paths, coverage matching, and the
    /// advance auto-`worked` derivation share — so observation joins them
    /// onto the workspace root, never onto the medium pointer (which the
    /// ids already embed).
    fn single_path_medium_root(&self, mem: &str) -> Option<PathBuf> {
        let workspace_root = self.workspace_root.as_deref()?;
        let mut mediums = self
            .pipeline_configs()
            .mediums
            .iter()
            .filter(|r| r.mem == mem);
        let first = mediums.next()?;
        if mediums.next().is_some() {
            return None; // ambiguous — an anchor names no medium
        }
        let caps = crate::binding::medium_capabilities(first.config.medium_type);
        if caps.anchor_namespace != "path" {
            return None; // only plain working-tree path mediums are observable here
        }
        Some(workspace_root.to_path_buf())
    }

    /// Reverse anchor lookup: every `(entity_id, anchor)` across all mems
    /// whose anchor references `artifact_path`. This is the query the
    /// rebuilt check-realization hook consumes — given the file an agent
    /// just edited, which entities anchored to it. A `span`/`file`/`tree`
    /// anchor references the path when its base path (locator suffix
    /// `@commit` / `#span` stripped) equals the path, or — for a `tree`
    /// grain — when the path lies under the tree. Path-shaped grains only;
    /// `url` / `entity` anchors are matched by exact base equality.
    pub fn anchors_referencing_artifact(
        &self,
        artifact_path: &str,
    ) -> Vec<(EntityId, crate::anchor::Anchor)> {
        let mut out = Vec::new();
        for mount in &self.mounts {
            let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
                continue;
            };
            let Ok(sc) = crate::anchor::AnchorSidecar::from_bytes(&bytes) else {
                continue;
            };
            for (eid, anchors) in &sc.entities {
                for a in anchors {
                    if anchor_references_path(a, artifact_path) {
                        out.push((EntityId(eid.clone()), a.clone()));
                    }
                }
            }
        }
        out
    }

    /// Every `(entity_id, resolved anchor)` in `mem`, read from its anchors
    /// sidecar once and each paired with its **live** resolution state (the
    /// same observation [`Self::entity_anchors_resolved`] produces per entity,
    /// computed here mem-wide in a single sidecar read). Empty for an unknown
    /// mem, a backend that persists no anchors, or a mem with none.
    ///
    /// Additive read surface: the durable data is unchanged; `state` is the
    /// [`crate::anchor::resolve_anchor`] outcome against an observation the
    /// engine produces for a single `path`-namespace medium, or `None` when
    /// unobserved (never fabricated). The verify pipeline consumes it to
    /// adjudicate a mem's anchors against the source; audit/health can reuse it.
    pub fn mem_anchors_resolved(&self, mem: &str) -> Vec<(EntityId, ResolvedAnchor)> {
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == mem) else {
            return Vec::new();
        };
        let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
            return Vec::new();
        };
        let Ok(sc) = crate::anchor::AnchorSidecar::from_bytes(&bytes) else {
            return Vec::new();
        };
        let root = self.single_path_medium_root(mem);
        let mut out = Vec::new();
        for (eid, anchors) in &sc.entities {
            for anchor in anchors {
                let observed = root.as_deref().and_then(|r| observe_path_anchor(r, anchor));
                let (state, observed_hash) = match observed {
                    Some((state, hash)) => (Some(state), hash),
                    None => (None, None),
                };
                out.push((
                    EntityId(eid.clone()),
                    ResolvedAnchor {
                        anchor: anchor.clone(),
                        state,
                        observed_hash,
                    },
                ));
            }
        }
        out
    }

    /// Mem names the engine knows about, in declaration order.
    /// Cheap; useful for callers that need to enumerate before
    /// dispatching by mem.
    pub fn mem_names(&self) -> Vec<&str> {
        self.mounts.iter().map(|m| m.mount.mem.as_str()).collect()
    }

    /// Public-shape mount record for `mem`, or `None` for an unknown
    /// mem.
    ///
    /// Surfaces the operator-facing
    /// [`crate::workspace::Mount`] (mem name, schema pin, storage
    /// reference, capability, lifecycle, cross_linkable) so MCP / CLI
    /// handlers can branch on backend-specific shapes via
    /// [`crate::workspace::MountStorage`] when they need accessors
    /// that don't make sense on every backend (e.g. gitdir / branch
    /// for `memstead_health { include_config: true }`'s git-class
    /// payload). Backends that want the equivalent of full's
    /// `engine.gitdir_for(mem)` match
    /// `engine.mount(mem).map(|m| &m.storage)` against
    /// `MountStorage::GitBranch { gitdir, branch }` and walk
    /// directly — keeps the engine surface backend-neutral.
    ///
    /// Counterpart to [`Self::mem_names`] which lists every mount.
    pub fn mount(&self, mem: &str) -> Option<&crate::workspace::Mount> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .map(|m| &m.mount)
    }

    /// Orphan count attributed to each mem's pinned schema, over the
    /// given `orphan_ids` (the caller pre-filters them by any mem scope).
    /// Lets a health surface show that ingest-mem isolates (orphans by
    /// design) and code-mem debt land in different schema buckets rather
    /// than one blended, misleading total. Mems with no settled pin
    /// bucket under the empty string.
    pub fn orphans_by_schema(
        &self,
        orphan_ids: &[EntityId],
    ) -> std::collections::BTreeMap<String, usize> {
        let mut by_schema = std::collections::BTreeMap::new();
        for id in orphan_ids {
            let schema = self
                .store()
                .get(id)
                .and_then(|e| self.mount(&e.mem))
                .and_then(|m| m.schema.as_ref().map(|s| s.as_display()))
                .unwrap_or_default();
            *by_schema.entry(schema).or_insert(0) += 1;
        }
        by_schema
    }

    /// Community count attributed to each schema across `mems`: a cluster
    /// counts toward every schema whose mems it touches, so these figures
    /// can sum above the global community count — the same "touches"
    /// semantic as the mem-scoped count. Per-schema dedup keeps a cluster
    /// touching two mems of one schema from being counted twice.
    pub fn communities_by_schema(
        &self,
        mems: &[String],
    ) -> std::collections::BTreeMap<String, usize> {
        let louvain = self.communities();
        let mut buckets: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
            std::collections::BTreeMap::new();
        for name in mems {
            let schema = self
                .mount(name)
                .and_then(|m| m.schema.as_ref().map(|s| s.as_display()))
                .unwrap_or_default();
            let clusters = crate::graph::community::clusters_in_mem(self.store(), louvain, name);
            buckets.entry(schema).or_default().extend(clusters);
        }
        buckets
            .into_iter()
            .map(|(schema, set)| (schema, set.len()))
            .collect()
    }

    /// All mounts the engine knows about, in declaration order.
    /// Counterpart to [`Self::mem_names`] when the caller needs
    /// the full mount shape (e.g. to enumerate by storage variant).
    pub fn mounts(&self) -> Vec<&crate::workspace::Mount> {
        self.mounts.iter().map(|m| &m.mount).collect()
    }

    /// Names of mems whose mount declares
    /// [`crate::workspace::MountCapability::Write`], in declaration
    /// order. Convenience over `mounts().iter().filter(...).map(...)`
    /// for handlers that gate by writable status (`memstead_health`,
    /// `memstead_overview`'s mem roster, the lifecycle tools'
    /// candidate list). Read-only mounts (archive backends) are
    /// excluded.
    pub fn writable_mem_names(&self) -> Vec<&str> {
        self.mounts
            .iter()
            .filter(|m| m.mount.capability == MountCapability::Write)
            .map(|m| m.mount.mem.as_str())
            .collect()
    }

    /// The default writable mem — the target a mutation lands in when
    /// it omits `mem`. `None` when no writable mem is mounted.
    ///
    /// Defined as the **first writable mount in declaration order**, i.e.
    /// the seed / earliest-created writable mem. This is a *stable*
    /// designation, not a function of the current name set: new mems
    /// register via `register_writable_mem`, which pushes onto the end
    /// of the mount list (and `mounts.json` preserves that order across
    /// reboots), so creating an additional mem never moves the default
    /// — even one whose name sorts ahead alphabetically. Deleting the
    /// current default promotes the next-earliest writable mem; that is
    /// the only thing that shifts it. Both the MCP `resolve_mem` and the
    /// CLI's omitted-`--mem` path resolve through here so the two
    /// surfaces always agree (the
    /// pre-fix MCP path read `writable_mems().iter().next()` off an
    /// unordered `HashSet`, which silently retargeted writes when a second
    /// mem appeared).
    pub fn default_writable_mem(&self) -> Option<&str> {
        self.mounts
            .iter()
            .find(|m| m.mount.capability == MountCapability::Write)
            .map(|m| m.mount.mem.as_str())
    }

    /// On-disk folder path for a folder-backed mount, or `None` for
    /// any other backend (git-branch, archive) or unknown mem.
    /// Convenience over `engine.mount(mem).map(|m| &m.storage)` +
    /// matching on `MountStorage::Folder { path }`. Used by
    /// handlers that need a filesystem path for a folder mem
    /// (e.g. `memstead_health { include_config: true }`'s
    /// `mems[].vcs.worktree` field for folder mounts).
    pub fn folder_path_for_mem(&self, mem: &str) -> Option<&Path> {
        match self.mount(mem).map(|m| &m.storage) {
            Some(crate::workspace::MountStorage::Folder { path }) => Some(path.as_path()),
            _ => None,
        }
    }

    /// Runtime snapshot of writable / visible mems. Handlers that
    /// need the writable roster (`memstead_health`'s `writable_mems` /
    /// `read_mems`), per-mem origin tag (`include_config:
    /// true`'s `mems[].origin`), or visibility check
    /// (`memstead_overview`'s mem list, the lifecycle tools' collision
    /// guard) consume the router here. Returned by reference — the
    /// `Arc` is held on the engine; callers that need a clonable
    /// handle can `Arc::clone` the engine's field directly when that
    /// surface arrives.
    pub fn mem_router(&self) -> &MemRouterSnapshot {
        &self.mem_router
    }

    /// Resolve the gitdir for a writable mem. Used by `memstead_health
    /// { include_config: true }` to surface per-mem `vcs.gitdir`
    /// so outer-repo bookkeeping clients can `git -C <gitdir>` per
    /// mem without hardcoding the layout.
    ///
    /// - `EngineError::UnknownMem` when the name does not resolve.
    /// - `EngineError::Mem` when the mount's storage is not
    ///   git-branch-backed (folder, archive — they have no gitdir).
    pub fn gitdir_for(&self, mem_name: &str) -> Result<PathBuf, EngineError> {
        let m = self
            .mount(mem_name)
            .ok_or_else(|| EngineError::UnknownMem(mem_name.to_string()))?;
        match &m.storage {
            MountStorage::GitBranch { gitdir, .. } => Ok(gitdir.clone()),
            MountStorage::Folder { .. } | MountStorage::Archive { .. } | MountStorage::InMemory => {
                Err(EngineError::Mem(format!(
                    "mem '{mem_name}' has no resolved gitdir"
                )))
            }
        }
    }

    /// Resolve the worktree for a writable mem. Used by
    /// `memstead_health { include_config: true }` to surface per-mem
    /// `vcs.worktree`.
    ///
    /// - `EngineError::UnknownMem` when the name does not resolve.
    /// - `EngineError::Mem` when the mount's backend has no
    ///   worktree concept (git-branch with no working tree, archive).
    ///
    /// Folder mounts surface their on-disk path. Git-branch mounts
    /// follow the `dir: Some(...)` composition pattern: when the
    /// workspace root contains a folder named after the mem with a
    /// `.memstead/config.json` marker, that folder is the worktree
    /// (disk-shape composition). Otherwise — pure mem-repo-backed
    /// — return Err.
    pub fn worktree_for(&self, mem_name: &str) -> Result<PathBuf, EngineError> {
        let m = self
            .mount(mem_name)
            .ok_or_else(|| EngineError::UnknownMem(mem_name.to_string()))?;
        match &m.storage {
            MountStorage::Folder { path } => Ok(path.clone()),
            MountStorage::GitBranch { .. } => {
                if let Some(root) = self.workspace_root.as_deref() {
                    let candidate = root.join(mem_name);
                    if candidate
                        .join(crate::mem::MEM_META_DIR)
                        .join("config.json")
                        .is_file()
                    {
                        return Ok(candidate.canonicalize().unwrap_or(candidate));
                    }
                }
                Err(EngineError::Mem(format!(
                    "mem '{mem_name}' has no working tree (mem-repo-backed)"
                )))
            }
            MountStorage::Archive { .. } => Err(EngineError::Mem(format!(
                "mem '{mem_name}' is archive-backed and has no worktree"
            ))),
            MountStorage::InMemory => Err(EngineError::Mem(format!(
                "mem '{mem_name}' is in-memory and has no worktree"
            ))),
        }
    }

    /// Per-mem `.memstead/config.json` payload, when available. Used
    /// by `memstead_health { include_config: true }` to surface the
    /// opaque `write_guidance` map and the catch-all `extra` fields
    /// per mem.
    ///
    /// Folder-backed mounts return `Some(&MemConfig)` when
    /// `<path>/.memstead/config.json` parsed cleanly at construction.
    /// Git-branch and archive backends return `None` until the
    /// read-from-storage-backend path lifts (the V1 unified engine
    /// loads configs only from folder layouts; the file lives
    /// inside the gitdir / archive for the other backends and
    /// needs a backend-level read primitive).
    ///
    /// Unknown mem names return `None` (no error variant — the
    /// accessor is intentionally lenient because memstead_health emits
    /// an empty detail block per missing config rather than
    /// aborting the call).
    pub fn mem_config_for(&self, mem: &str) -> Option<&memstead_schema::config::MemConfig> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| m.mem_config.as_ref())
    }

    /// The authoring-provenance payload an installed mem carries, read
    /// from the archive's `.memstead/provenance.json` at construction.
    /// `None` when the mem carries none (a pre-provenance archive, a
    /// runtime-created mem, or a backend that does not surface one) —
    /// the read path reports provenance as absent. Unknown mem names
    /// return `None`.
    pub fn archive_provenance_for(&self, mem: &str) -> Option<&memstead_schema::ArchiveProvenance> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| m.archive_provenance.as_ref())
    }

    /// Iterate `(mem_name, &MemConfig)` for every mount whose
    /// mem-config payload loaded at construction. Used by callers
    /// that walk every writable mount's config (`memstead health`'s
    /// per-mem dump, the workspace-dump CLI). The yielded `&str` is
    /// the authoritative mem leaf from the mount record.
    ///
    /// Folder-backed mounts yield when their `.memstead/config.json`
    /// parsed cleanly. Git-branch and archive backends are silent in
    /// V1 (the same deferred-read-from-storage gap that
    /// [`Self::mem_config_for`] documents).
    pub fn mem_configs_named(
        &self,
    ) -> impl Iterator<Item = (&str, &memstead_schema::config::MemConfig)> {
        self.mounts
            .iter()
            .filter_map(|m| m.mem_config.as_ref().map(|c| (m.mount.mem.as_str(), c)))
    }

    /// Resolved `Arc<Schema>` for a writable mem by name. `None`
    /// when the name is not a registered mount.
    ///
    /// Cheap — `Arc::clone` over the per-mem schema map. Resolved
    /// schemas are stored in `HashMap<String, Arc<Schema>>` so the
    /// lookup is a single hash hit + clone.
    pub fn schema_for(&self, mem: &str) -> Option<std::sync::Arc<memstead_schema::Schema>> {
        self.schemas.get(mem).cloned()
    }

    /// Cached current branch-tip cursor (typically a 40-char hex
    /// SHA for git-branch backends; `None` for fresh mems or
    /// backends that don't track a head — folder / archive).
    ///
    /// The value is the per-mount `last_known_head`, seeded at
    /// construction by `backend.current_head()` and refreshed by
    /// [`Self::reload_if_stale`] / mutation paths after a
    /// successful commit.
    ///
    /// - `EngineError::UnknownMem` when the name does not resolve.
    pub fn mem_head_sha(&self, mem_name: &str) -> Result<Option<String>, EngineError> {
        let m = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem_name)
            .ok_or_else(|| EngineError::UnknownMem(mem_name.to_string()))?;
        Ok(m.last_known_head.clone())
    }

    /// Whether a sibling writer has advanced this mem's backend past
    /// the engine's cached `last_known_head` — a read-only drift probe
    /// that does **not** reload (unlike [`Self::reload_if_stale`]). One
    /// `backend.current_head()` read compared against the cached cursor;
    /// the comparison clears once the engine re-reads (a `reload` /
    /// `reload_if_stale` refreshes `last_known_head` to the live tip).
    ///
    /// Only git-branch backends track a head, so folder / archive /
    /// in-memory mounts always report `false`. A backend that errors on
    /// the probe (transient refdb hiccup) reports `false` rather than
    /// surfacing the error — drift is advisory, and the next real
    /// operation's reload path is the authoritative sync.
    ///
    /// - `EngineError::UnknownMem` when the name does not resolve.
    pub fn mem_drifted(&self, mem_name: &str) -> Result<bool, EngineError> {
        let m = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem_name)
            .ok_or_else(|| EngineError::UnknownMem(mem_name.to_string()))?;
        let live = m.backend.current_head().ok().flatten();
        Ok(live != m.last_known_head)
    }

    /// Workspace root the engine booted from, when one is known.
    /// `None` for engines built directly from a mount list (tests,
    /// ad-hoc consumers). Set by [`Self::from_workspace_root`] and
    /// the full counterpart.
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    /// Typed warnings surfaced during mem load — drift findings
    /// the loader pipeline collects per entity. Empty for V1; the
    /// accessor surfaces them so handlers can merge into health
    /// summaries uniformly.
    pub fn load_warnings(&self) -> &[WarningHint] {
        &self.load_warnings
    }

    // ---------------------------------------------------------------
    // Read-side delegates onto the kernel ops/graph functions.
    //
    // The mem-router engine exposed each of these directly so the
    // MCP layer could call them without reaching into the store. The
    // unified engine mirrors that surface so the MCP migration is a
    // straight rename rather than a re-architecture.
    //
    // Multi-mem cache strategy: per-mem community detection and
    // per-mem search indexes are unnecessary at this layer — the
    // engine-wide store already carries every mount's edges; Louvain
    // and tantivy run once across the union. `mem_schemas` for
    // health/search is the engine's existing `schemas` field as-is.
    // ---------------------------------------------------------------

    /// Lazy community-detection cache. First call runs Louvain
    /// against the current store using one pinned schema for
    /// `community.{resolution, seed}` and the per-rel weights.
    /// Subsequent calls return the cached result. Mutations invalidate
    /// the cache via [`Self::invalidate_communities`].
    ///
    /// One detection run per engine. The partition is workspace-global,
    /// so it needs a single source for the Louvain parameters; that
    /// source is the schema of the lexicographically-first mem name —
    /// a stable key, so the partition is deterministic across processes
    /// even when mounts pin heterogeneous schemas. For a single-schema
    /// workspace every mem's schema is identical, so the choice of
    /// key is immaterial there.
    pub fn communities(&self) -> &LouvainOutput {
        self.community_memo.get_or_init(|| {
            // Select the parameter schema by a stable key (smallest
            // mem name) rather than unordered-map iteration, so the
            // partition does not vary between processes. Fall back to
            // the builtin default for the empty-mounts case (caller
            // still gets a valid empty Louvain result against an empty
            // store).
            let schema = self
                .schemas
                .iter()
                .min_by(|a, b| a.0.cmp(b.0))
                .map(|(_, s)| s.clone())
                .unwrap_or_else(Schema::builtin_default);
            let manifest = &schema.manifest;
            let resolution = manifest.community.resolution;
            let seed = manifest.community.seed;
            let schema_for_weights = schema.clone();
            detect_communities(&self.store, resolution, seed, move |rel_type| {
                schema_for_weights
                    .manifest
                    .relationships
                    .definitions
                    .iter()
                    .find(|d| d.name == rel_type)
                    .map(|d| d.default_weight as f64)
                    .unwrap_or(1.0)
            })
        })
    }

    /// Drop the cached community detection result.
    pub fn invalidate_communities(&mut self) {
        self.community_memo = OnceCell::new();
    }

    /// Real entities with no incoming or outgoing edges.
    pub fn orphans(&self) -> Vec<EntityId> {
        crate::graph::query::find_orphans(&self.store)
    }

    /// Stub entities with their referencer ids.
    pub fn stubs(&self) -> Vec<(EntityId, Vec<EntityId>)> {
        crate::graph::query::find_stubs(&self.store)
    }

    /// Top `limit` entities by total degree.
    pub fn most_connected(&self, limit: usize) -> Vec<crate::graph::query::Connectivity> {
        crate::graph::query::most_connected(&self.store, limit)
    }

    /// Entities whose type's `required_outgoing` blocks are not yet
    /// satisfied. `mem_filter = None` scans every mem; `Some(v)`
    /// scans only that mem.
    pub fn missing_required_outgoing(
        &self,
        mem_filter: Option<&str>,
    ) -> Vec<crate::ops::health::MissingRequiredOutgoingReport> {
        crate::ops::health::collect_missing_required_outgoing(
            &self.store,
            mem_filter,
            &self.schemas,
        )
    }

    /// Conformance-axis integrity findings for one mem — which
    /// entities a write would refuse under the effective schema, and
    /// why. `target_schema = None` lints against the mem's current
    /// pin; `Some(ref)` lints against that schema instead (resolved
    /// among mem-pinned, workspace, and built-in schemas).
    pub fn conformance_findings(
        &self,
        mem: &str,
        target_schema: Option<&memstead_schema::SchemaRef>,
    ) -> Result<Vec<crate::ops::integrity::IntegrityFinding>, EngineError> {
        let pinned = self
            .schemas
            .get(mem)
            .ok_or_else(|| EngineError::UnknownMem(mem.to_string()))?;
        let effective: Arc<Schema> = match target_schema {
            None => pinned.clone(),
            Some(target) => self.resolve_schema_by_ref(target).ok_or_else(|| {
                let consulted: Vec<_> = self
                    .workspace_schemas
                    .iter()
                    .chain(self.builtin_schemas.iter())
                    .cloned()
                    .collect();
                EngineError::SchemaNotFound {
                    mem: mem.to_string(),
                    pin: target.as_display(),
                    sources: crate::engine::error::SchemaSourceDiagnostic::for_failed_pin(
                        &target.name,
                        &target.version,
                        &consulted,
                    ),
                }
            })?,
        };
        Ok(crate::ops::integrity::conformance_findings(
            &self.store,
            mem,
            &effective,
            &self.schemas,
        ))
    }

    /// Resolve an exact `name@version` ref against every schema this
    /// engine can see: mem-pinned, workspace-authored, built-in.
    /// `None` when no loaded schema matches.
    pub(crate) fn resolve_schema_by_ref(
        &self,
        target: &memstead_schema::SchemaRef,
    ) -> Option<Arc<Schema>> {
        self.schemas
            .values()
            .chain(self.workspace_schemas.iter())
            .chain(self.builtin_schemas.iter())
            .find(|s| {
                let (name, version) = s.id();
                name == target.name && version == target.version
            })
            .cloned()
    }

    /// The mem's `Mount.schema` expectation assertion, when set.
    /// `None` for unknown mems *and* for mems whose mount carries no
    /// assertion (the authoritative pin then lives in the backend
    /// config; the resolved active schema, not this, is the effective pin).
    pub fn schema_pin(&self, mem: &str) -> Option<memstead_schema::SchemaRef> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| m.mount.schema.clone())
    }

    /// The mem's in-flight migration target, when dual-pin state is
    /// active. `None` for settled or unknown mems.
    pub fn migration_target(&self, mem: &str) -> Option<memstead_schema::SchemaRef> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| m.mount.migration_target.clone())
    }

    /// Consistency-axis integrity findings for one mem — the
    /// pre-existing graph-coherence categories (dangling links, stubs)
    /// projected into the `{ id, axis, code, detail }` finding shape.
    pub fn consistency_findings(
        &self,
        mem: &str,
    ) -> Result<Vec<crate::ops::integrity::IntegrityFinding>, EngineError> {
        if !self.schemas.contains_key(mem) {
            return Err(EngineError::UnknownMem(mem.to_string()));
        }
        Ok(crate::ops::integrity::consistency_findings(
            &self.store,
            mem,
        ))
    }

    /// Engine-wide health summary across every mount.
    pub fn health(&self) -> crate::ops::HealthSummary {
        let fallback = engine_fallback_type();
        let mut summary =
            crate::ops::health::compute_health(&self.store, fallback.as_ref(), &self.schemas);
        // Merge in load-time drift warnings so every caller of
        // Engine::health — MCP handler, Swift FFI, direct CLI —
        // sees the SuspiciousNestedPrefix / DuplicateSectionHeading
        // findings without reaching into private engine state. The
        // MCP handler further appends request-scoped warnings on
        // top. Mirrors full's merge.
        if !self.load_warnings.is_empty() {
            let mut merged = self.load_warnings.clone();
            merged.append(&mut summary.warnings);
            summary.warnings = merged;
        }
        // Surface OUTER_REPO_NOT_IGNORING_MEM_REPO when the
        // workspace is embedded inside a git repository whose
        // .gitignore does not list `mem-repo/`. Skipped when
        // workspace_root is unset (engine built ad-hoc from a mount
        // list).
        if let Some(root) = self.workspace_root.as_deref()
            && let Some(outer) = crate::workspace_root::find_enclosing_git_repo(root)
            && !crate::workspace_root::outer_repo_ignores_mem_repo(&outer, root)
        {
            summary
                .warnings
                .push(WarningHint::OuterRepoNotIgnoringMemRepo {
                    outer_repo_root: outer.display().to_string(),
                    workspace_root: root.display().to_string(),
                });
        }
        summary
    }

    /// Engine-wide [`crate::ops::Status`] across every mount — the graph
    /// counts behind `memstead status` (renamed from `stats` with the
    /// command, D11; fields unchanged).
    pub fn status(&self) -> crate::ops::Status {
        let mut types_in_use: Vec<String> = self
            .store
            .all_entities()
            .filter(|e| !e.stub && !e.entity_type.is_empty())
            .map(|e| e.entity_type.clone())
            .collect();
        types_in_use.sort();
        types_in_use.dedup();

        let mut edge_types: HashMap<String, usize> = HashMap::new();
        for id in self.store.all_ids() {
            for edge in self.store.outgoing(id) {
                *edge_types.entry(edge.rel_type.clone()).or_insert(0) += 1;
            }
        }

        crate::ops::Status {
            entity_count: self.store.all_entities().filter(|e| !e.stub).count(),
            edge_count: self.store.edge_count(),
            edge_types,
            community_count: self.communities().count,
            mem_count: self.mounts.len(),
            types_in_use,
        }
    }

    /// Build a [`ContextResult`] for `id`: the community cluster id
    /// (or `None` when the entity is a stub or not present), plus the
    /// outgoing + incoming neighbour lists.
    pub fn context(&self, id: &EntityId) -> Option<ContextResult> {
        let entity = self.store.get(id)?;
        let community = self
            .communities()
            .entity_cluster_map
            .get(id.as_ref())
            .cloned();
        let mut neighbors = Vec::new();
        for edge in self.store.outgoing(id) {
            if let Some(target) = self.store.get(&edge.target) {
                neighbors.push(NeighborInfo {
                    id: target.id.clone(),
                    title: target.title.clone(),
                    relationship: edge.rel_type.clone(),
                    direction: Direction::Outgoing,
                });
            }
        }
        for edge in self.store.incoming(id) {
            if let Some(source) = self.store.get(&edge.from) {
                neighbors.push(NeighborInfo {
                    id: source.id.clone(),
                    title: source.title.clone(),
                    relationship: edge.rel_type.clone(),
                    direction: Direction::Incoming,
                });
            }
        }
        Some(ContextResult {
            entity_id: entity.id.clone(),
            community,
            neighbors,
        })
    }

    /// Lazily-built per-mem search index map. The map carries one
    /// entry per writable mem. Build cost scales with entity count;
    /// expect hundreds-of-ms for thousand-entity workspaces. Not
    /// available on `wasm32` targets — search lives behind the bridge
    /// (see [`Self::search`] for the typed refuse).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn search_indexes(&self) -> &HashMap<String, MemIndex> {
        self.search_indexes_memo
            .get_or_init(|| build_all(&self.store, &self.schemas))
    }

    /// Drop the cached per-mem search index map. No-op on `wasm32`
    /// where no index exists; the method stays present so mutation
    /// hooks can call it unconditionally.
    pub fn invalidate_search_indexes(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.search_indexes_memo = OnceCell::new();
        }
    }

    /// Filter the in-memory store by metadata only (no text match).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn list(&self, scope: &SearchScope) -> crate::ops::ListResult {
        let fallback = engine_fallback_type();
        crate::ops::search::list(&self.store, scope, fallback.as_ref(), &self.schemas)
    }

    /// Run a search against the lazily-built index map. Returns
    /// [`EngineError::SearchUnavailable`] on `wasm32` targets — browser
    /// consumers route search to the bridge; the local
    /// engine never builds a tantivy index in WASM. Native targets get
    /// the same shape as before, wrapped in `Ok`.
    pub fn search(&self, scope: &SearchScope) -> Result<SearchResult, EngineError> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = scope;
            return Err(EngineError::SearchUnavailable);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let fallback = engine_fallback_type();
            Ok(crate::ops::search::search(
                &self.store,
                scope,
                fallback.as_ref(),
                self.search_indexes(),
                &self.schemas,
            ))
        }
    }

    /// All mem-relative entity paths under `mem`. Delegates to
    /// the backend's `list_entities`. Order is backend-defined.
    pub fn list_entities(&self, mem: &str) -> Result<Vec<PathBuf>, EngineError> {
        let m = self.find_mount(mem)?;
        m.backend.list_entities().map_err(EngineError::Backend)
    }

    /// Raw bytes for a single entity (`Ok(None)` if absent).
    pub fn read_entity(&self, mem: &str, rel_path: &Path) -> Result<Option<Vec<u8>>, EngineError> {
        let m = self.find_mount(mem)?;
        m.backend
            .read_entity(rel_path)
            .map_err(EngineError::Backend)
    }

    /// Provenance entries for `mem` since `cursor`. Cursor shape is
    /// backend-specific (RFC-3339 timestamp for folder, commit SHA for
    /// git-branch); `None` means "from the beginning".
    pub fn read_provenance(
        &self,
        mem: &str,
        cursor: Option<&str>,
    ) -> Result<Vec<Provenance>, EngineError> {
        let m = self.find_mount(mem)?;
        m.backend
            .read_provenance(cursor)
            .map_err(EngineError::Backend)
    }

    /// Capability declared on the mount for `mem`. Surfaced for
    /// callers that need to gate before dispatching a write — the
    /// engine itself does not yet enforce capability (mutation paths
    /// land in a later session).
    pub fn capability(&self, mem: &str) -> Result<crate::workspace::MountCapability, EngineError> {
        let m = self.find_mount(mem)?;
        Ok(m.mount.capability)
    }

    /// Returns `true` when `from`'s source mem is mounted with
    /// [`crate::workspace::MountCapability::ReadOnly`]. Returns
    /// `false` for Write-Mems and for mems whose mount is absent
    /// from the router (no mount → no ReadOnly assertion can be
    /// made; the absence is treated as not-ReadOnly so consumers
    /// don't trip on transient lookup misses).
    ///
    /// Plan body §"Single edge source in the store" specifies this
    /// helper as the derived-on-demand alternative to adding a new
    /// field on [`crate::store::Edge`]. Strict-invariant validators
    /// and surfaces that want to highlight cross-mount references
    /// call this rather than pattern-matching on a per-edge marker.
    /// The information is fully derivable from the current mount
    /// roster, so no new state needs to live on the edge itself.
    pub fn edge_is_from_readonly(&self, from: &EntityId) -> bool {
        match self.capability(from.mem()) {
            Ok(crate::workspace::MountCapability::ReadOnly) => true,
            Ok(crate::workspace::MountCapability::Write) | Err(_) => false,
        }
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
            .ok_or_else(|| EngineError::UnknownMem(mem.to_string()))
    }
}

/// The base path of an anchor artifact ref — the locator suffixes a
/// medium may append (`@<commit>`, `#<span>`) stripped so the reverse
/// lookup compares paths, not versioned/located refs.
fn anchor_base_path(artifact: &str) -> &str {
    let cut = artifact.find(['@', '#']).unwrap_or(artifact.len());
    &artifact[..cut]
}

/// Whether `anchor` references `path`. `tree`-grain anchors match `path`
/// itself and anything beneath the tree; every other grain matches by
/// exact base-path equality.
/// A stored anchor paired with its live resolution state, when observable.
/// See [`Engine::entity_anchors_resolved`] for how `state` is produced and
/// when it is `None` (unobserved, never fabricated).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ResolvedAnchor {
    /// The durable anchor record (flattened on the wire so the resolved shape
    /// is the stored anchor plus a `state` field).
    #[serde(flatten)]
    pub anchor: crate::anchor::Anchor,
    /// The live resolution state, or `None` when the engine could not observe
    /// the source artifact this pass (non-path medium, ambiguous / absent
    /// medium, no workspace root, or a non-filesystem grain).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::anchor::AnchorState>,
    /// The prepared-content hash the observation computed this pass —
    /// present only for a hash-bearing (`anchored` / `derived`) `file` /
    /// `span` anchor whose artifact resolved to a readable file. The verify
    /// pass's backfill leg records it onto a hash-less anchor. Engine-internal
    /// observation detail, deliberately not serialized: the wire shape stays
    /// the stored anchor plus `state`.
    #[serde(skip)]
    pub observed_hash: Option<String>,
}

/// Observe a single path-namespace anchor against `root` (its medium's
/// filesystem root) and resolve its live state plus — for a present
/// hash-bearing (`anchored` / `derived`) `file` / `span` anchor — the
/// artifact's **prepared-content hash**
/// ([`crate::anchor::prepared_content_hash`]). `None` when the anchor's
/// grain does not reference a filesystem path.
///
/// The computed hash is what lets [`crate::anchor::resolve_anchor`]
/// adjudicate `drifted` vs `resolves` deterministically against the recorded
/// hash. A `span` anchor hashes its whole containing file (the span locator
/// selects within it; the file is the hashed unit); a `tree` grain has no
/// prepared form this cycle and observes no hash; a read failure likewise
/// observes no hash — those resolve `recheck`, never a fabricated `drifted`.
/// Non-hash classes (`authored` / `informed-by`) skip the read entirely, so
/// an anchor-less or hash-free mem pays no observation cost.
fn observe_path_anchor(
    root: &Path,
    anchor: &crate::anchor::Anchor,
) -> Option<(crate::anchor::AnchorState, Option<String>)> {
    use crate::anchor::AnchorGrain;
    match anchor.grain {
        AnchorGrain::Span | AnchorGrain::File | AnchorGrain::Tree => {}
        AnchorGrain::Url | AnchorGrain::Entity => return None,
    }
    let base = anchor_base_path(&anchor.artifact);
    let path = root.join(base);
    if !path.exists() {
        return Some((
            crate::anchor::resolve_anchor(anchor, &crate::anchor::ArtifactObservation::Absent),
            None,
        ));
    }
    let current_hash = if anchor.class.is_hash_bearing()
        && matches!(anchor.grain, AnchorGrain::File | AnchorGrain::Span)
        && path.is_file()
    {
        std::fs::read(&path)
            .ok()
            .map(|bytes| crate::anchor::prepared_content_hash(&bytes))
    } else {
        None
    };
    let observation = crate::anchor::ArtifactObservation::Present {
        current_hash: current_hash.clone(),
    };
    Some((
        crate::anchor::resolve_anchor(anchor, &observation),
        current_hash,
    ))
}

fn anchor_references_path(anchor: &crate::anchor::Anchor, path: &str) -> bool {
    let base = anchor_base_path(&anchor.artifact);
    if base == path {
        return true;
    }
    if anchor.grain == crate::anchor::AnchorGrain::Tree {
        let prefix = base.strip_suffix('/').unwrap_or(base);
        return path.starts_with(&format!("{prefix}/"));
    }
    false
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use crate::backend::{BackendError, MemBackend};
    use crate::engine::test_helpers::*;
    use crate::engine::{Engine, EngineError, RelateEntityArgs};
    use crate::entity::EntityId;
    use crate::ops::{Direction, SearchScope, WarningHint};
    use crate::provenance::Provenance;
    use crate::storage::{ArchiveBackend, FilesystemMemWriter, MemWriter};

    use crate::vcs::CommitContext;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    /// `schema_origin` is the trust-classification authority: a built-in
    /// (or workspace-authored) schema is first-party; a schema whose
    /// `(name, version)` is in neither catalogue is third-party — the safe
    /// default for an origin the engine cannot vouch for.
    #[test]
    fn schema_origin_classifies_builtin_first_party_and_unknown_third_party() {
        use std::sync::Arc;

        use crate::render::OriginClass;

        let tmp = TempDir::new().unwrap();
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", tmp.path().to_path_buf()),
            Box::new(FilesystemMemWriter::new(tmp.path().to_path_buf())) as Box<dyn MemBackend>,
        )])
        .unwrap();

        // A built-in schema (the catalogue the engine resolved against).
        let builtin = engine.builtin_schemas()[0].clone();
        assert_eq!(
            engine.schema_origin(&builtin),
            OriginClass::FirstParty,
            "a built-in schema is first-party"
        );

        // A schema whose version is in no catalogue — a stand-in for a
        // schema that entered from outside the workspace. Same name, a
        // version the engine never loaded.
        let foreign = Arc::new(memstead_schema::Schema {
            manifest: builtin.manifest.clone(),
            version: semver::Version::new(99, 0, 0),
            types: builtin.types.clone(),
        });
        assert_eq!(
            engine.schema_origin(&foreign),
            OriginClass::ThirdParty,
            "a schema in neither catalogue classifies third-party (safe default)"
        );
    }

    /// `mem_origin_class` classifies a writable mount first-party (its
    /// content is authored in this workspace) and a read-only mount
    /// third-party (registry-installed read-mem or adopted foreign
    /// folder/clone — quoted, untrusted data). An unknown mem is
    /// third-party (the safe default).
    #[test]
    fn mem_origin_class_writable_first_party_readonly_third_party() {
        use crate::render::OriginClass;

        let tmp = TempDir::new().unwrap();
        // Writable folder mem.
        let writable_dir = tmp.path().join("writable");
        std::fs::create_dir_all(&writable_dir).unwrap();
        let writer = FilesystemMemWriter::new(writable_dir.clone());

        // Read-only archive mem.
        let body = "---\ntype: spec\n---\n# Ext\n\n## Identity\n\nFrom an archive.\n";
        let archive_path = build_archive(tmp.path(), "ext", &[("ext.md", body.as_bytes())]);

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("local", writable_dir),
                Box::new(writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("external", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap();

        assert_eq!(
            engine.mem_origin_class("local"),
            OriginClass::FirstParty,
            "a writable mount is first-party"
        );
        assert_eq!(
            engine.mem_origin_class("external"),
            OriginClass::ThirdParty,
            "a read-only mount is third-party"
        );
        assert_eq!(
            engine.mem_origin_class("no-such-mem"),
            OriginClass::ThirdParty,
            "an unknown mem is third-party (safe default)"
        );
    }

    /// `declare_mem_origin` lets the embedding deployment vouch for one
    /// read-only mount as first-party (the curated hosted read tier),
    /// overriding the writability inference for that mem only — sibling
    /// read-only mounts keep the safe third-party default.
    #[test]
    fn declared_origin_overrides_inference_per_mem() {
        use crate::render::OriginClass;

        let tmp = TempDir::new().unwrap();
        let body = "---\ntype: spec\n---\n# Ext\n\n## Identity\n\nFrom an archive.\n";
        let vouched_path = build_archive(tmp.path(), "vouched", &[("v.md", body.as_bytes())]);
        let other_path = build_archive(tmp.path(), "other", &[("o.md", body.as_bytes())]);

        let mut engine = Engine::from_mounts(vec![
            (
                archive_mount("vouched", vouched_path.clone()),
                Box::new(ArchiveBackend::new(vouched_path)) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("other", other_path.clone()),
                Box::new(ArchiveBackend::new(other_path)) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap();

        engine.declare_mem_origin("vouched", OriginClass::FirstParty);

        assert_eq!(
            engine.mem_origin_class("vouched"),
            OriginClass::FirstParty,
            "the deployment's declaration wins over the read-only inference"
        );
        assert_eq!(
            engine.mem_origin_class("other"),
            OriginClass::ThirdParty,
            "an undeclared sibling mount keeps the safe default"
        );
    }

    /// The adopt-gate: a non-built-in schema is first-party only once a
    /// writable mount pins it (the operator authors against it here).
    /// Pinned only by a read-only mount — a registry read-mem or an
    /// adopted foreign folder/clone — it stays third-party, so
    /// `memstead_schema` serves it structural-only.
    #[test]
    fn schema_origin_third_party_until_pinned_by_a_writable_mount() {
        use memstead_schema::SchemaRef;

        use crate::render::OriginClass;

        let manifest = r#"name: trust-test
version: 0.1.0
description: adopt-gate test schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let pin = SchemaRef::new("trust-test", semver::Version::new(0, 1, 0));

        let mk_engine = |cap: MountCapability| -> Engine {
            let tmp = TempDir::new().unwrap();
            let schemas_dir = tmp.path().join("schemas");
            std::fs::create_dir_all(&schemas_dir).unwrap();
            write_schema_files_with_default_type(&schemas_dir, "trust-test", manifest, &["doc"]);
            let mem_dir = tmp.path().join("mem");
            std::fs::create_dir_all(&mem_dir).unwrap();
            let mount = Mount {
                mem: "v".to_string(),
                schema: Some(pin.clone()),
                storage: MountStorage::Folder {
                    path: mem_dir.clone(),
                },
                capability: cap,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            };
            let backend = Box::new(FilesystemMemWriter::new(mem_dir)) as Box<dyn MemBackend>;
            // Keep `tmp` alive for the engine's lifetime by leaking it —
            // the test process is short-lived and the folder must outlast
            // the closure.
            std::mem::forget(tmp);
            Engine::from_mounts_with_schemas_dir(vec![(mount, backend)], Some(&schemas_dir))
                .unwrap()
        };

        // Read-only mount: the foreign schema is never adopted → third-party.
        let ro = mk_engine(MountCapability::ReadOnly);
        let schema = ro.schemas().get("v").expect("schema resolved").clone();
        assert_eq!(
            ro.schema_origin(&schema),
            OriginClass::ThirdParty,
            "a non-built-in schema pinned only by a read-only mount is third-party"
        );

        // Writable mount pinning the same schema: adopted → first-party.
        let rw = mk_engine(MountCapability::Write);
        let schema = rw.schemas().get("v").expect("schema resolved").clone();
        assert_eq!(
            rw.schema_origin(&schema),
            OriginClass::FirstParty,
            "a writable mount pinning the schema adopts it → first-party"
        );
    }

    /// Consumer read path: an installed (archive-backed) mem that ships
    /// a `.memstead/provenance.json` payload surfaces per-entity authoring
    /// provenance through `archive_provenance_for`. A noted entity carries
    /// its rationale; an entity authored without a note is absent from the
    /// payload and reads as provenance-absent (no fabricated value); the
    /// `history` disposition records that full history is not shipped.
    #[test]
    fn archive_provenance_surfaces_per_entity_and_reports_absence() {
        use memstead_schema::History;

        let tmp = TempDir::new().unwrap();
        let config = br#"{"format":3,"name":"seed","version":"0.1.0","schema":"default@1.0.0"}"#;
        let alpha = b"---\ntype: spec\n---\n# Alpha\n\n## Identity\n\na\n\n## Purpose\n\np\n";
        let beta = b"---\ntype: spec\n---\n# Beta\n\n## Identity\n\nb\n\n## Purpose\n\np\n";
        // alpha noted; beta deliberately absent from the payload.
        let provenance = br#"{"format":1,"history":"summarised","entities":{"alpha":{"rationale":"why alpha exists","kind":"create","timestamp":"2026-06-24T00:00:00Z","actor":"agent"}}}"#;
        let archive = build_archive(
            tmp.path(),
            "seed",
            &[
                (".memstead/config.json", config),
                ("alpha.md", alpha),
                ("beta.md", beta),
                (".memstead/provenance.json", provenance),
            ],
        );
        let engine = Engine::from_mounts(vec![(
            archive_mount("seed", archive.clone()),
            Box::new(ArchiveBackend::new(archive)) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let prov = engine
            .archive_provenance_for("seed")
            .expect("provenance payload read from the archive");
        assert_eq!(
            prov.history,
            History::Summarised,
            "history-not-shipped is observable"
        );
        assert_eq!(
            prov.entity("alpha").and_then(|r| r.rationale.as_deref()),
            Some("why alpha exists"),
            "noted entity surfaces its rationale"
        );
        assert!(
            prov.entity("beta").is_none(),
            "unnoted entity is absent (reported absent, not fabricated)"
        );
    }

    /// A pre-provenance archive (no `.memstead/provenance.json`) reads as
    /// provenance uniformly absent — the additive contract: a newer engine
    /// installing an old archive reports no provenance, never an error.
    #[test]
    fn archive_without_provenance_reports_absent() {
        let tmp = TempDir::new().unwrap();
        let config = br#"{"format":3,"name":"seed","version":"0.1.0","schema":"default@1.0.0"}"#;
        let alpha = b"---\ntype: spec\n---\n# Alpha\n\n## Identity\n\na\n\n## Purpose\n\np\n";
        let archive = build_archive(
            tmp.path(),
            "seed",
            &[(".memstead/config.json", config), ("alpha.md", alpha)],
        );
        let engine = Engine::from_mounts(vec![(
            archive_mount("seed", archive.clone()),
            Box::new(ArchiveBackend::new(archive)) as Box<dyn MemBackend>,
        )])
        .unwrap();
        assert!(
            engine.archive_provenance_for("seed").is_none(),
            "an archive without a provenance payload reports provenance absent"
        );
    }

    #[test]
    fn folder_mount_routes_reads_to_filesystem_backend() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        // MemWriter and MemBackend share method names; the
        // module-top `use` brings both into scope. Seed via fully-
        // qualified MemWriter calls so dot-syntax stays unambiguous.
        <FilesystemMemWriter as MemWriter>::write_entity(&writer, Path::new("a.md"), b"alpha")
            .unwrap();
        <FilesystemMemWriter as MemWriter>::commit(&writer, "seed", &CommitContext::internal())
            .unwrap();

        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let mut paths: Vec<String> = engine
            .list_entities("specs")
            .unwrap()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        paths.sort();
        assert_eq!(paths, vec!["a.md".to_string()]);

        assert_eq!(
            engine.read_entity("specs", Path::new("a.md")).unwrap(),
            Some(b"alpha".to_vec())
        );
    }

    #[test]
    fn heterogeneous_mounts_route_to_correct_backend() {
        let tmp = TempDir::new().unwrap();

        // Folder mem.
        let folder_dir = tmp.path().join("folder-mem");
        std::fs::create_dir_all(&folder_dir).unwrap();
        let folder_writer = FilesystemMemWriter::new(folder_dir.clone());
        <FilesystemMemWriter as MemWriter>::write_entity(
            &folder_writer,
            Path::new("local.md"),
            b"local",
        )
        .unwrap();
        <FilesystemMemWriter as MemWriter>::commit(
            &folder_writer,
            "seed",
            &CommitContext::internal(),
        )
        .unwrap();

        // Archive mem.
        let archive_path = build_archive(
            tmp.path(),
            "external",
            &[("ext.md", b"external"), ("dir/nested.md", b"nested")],
        );

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("local", folder_dir),
                Box::new(folder_writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("external", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path)),
            ),
        ])
        .unwrap();

        // Routes correctly by mem name.
        assert_eq!(engine.mem_names(), vec!["local", "external"]);
        assert_eq!(
            engine.read_entity("local", Path::new("local.md")).unwrap(),
            Some(b"local".to_vec())
        );
        assert_eq!(
            engine.read_entity("external", Path::new("ext.md")).unwrap(),
            Some(b"external".to_vec())
        );
        assert_eq!(
            engine
                .read_entity("external", Path::new("dir/nested.md"))
                .unwrap(),
            Some(b"nested".to_vec())
        );
        // Cross-routing: reading a path from the wrong mem → None
        // (the backend doesn't have it), not an error.
        assert_eq!(
            engine.read_entity("local", Path::new("ext.md")).unwrap(),
            None
        );
        assert_eq!(
            engine
                .read_entity("external", Path::new("local.md"))
                .unwrap(),
            None
        );
    }

    #[test]
    fn edge_is_from_readonly_classifies_every_edge_by_source_mount_capability() {
        // `engine.edge_is_from_readonly` is the derived-on-demand
        // alternative to adding a per-edge marker: construct a mixed
        // workspace (one Write-Mem + one ReadOnly archive with
        // cross-mem wiki-links) and walk every edge in the store,
        // asserting each edge's source-mount capability.
        let tmp = TempDir::new().unwrap();

        // Write folder mem `local` with a spec-shaped entity that
        // declares an explicit cross-mem relation into the archive
        // (under the alias model edges originate from `## Relationships`).
        let folder_dir = tmp.path().join("local-mem");
        std::fs::create_dir_all(&folder_dir).unwrap();
        let folder_writer = FilesystemMemWriter::new(folder_dir.clone());
        let local_md = b"---\ntype: spec\n---\n# Note\n\n## Identity\n\nsee [[external:archived]] for prior context.\n\n## Relationships\n\n- **REFERENCES**: [[external:archived]]\n";
        <FilesystemMemWriter as MemWriter>::write_entity(
            &folder_writer,
            Path::new("note.md"),
            local_md,
        )
        .unwrap();
        <FilesystemMemWriter as MemWriter>::commit(
            &folder_writer,
            "seed",
            &CommitContext::internal(),
        )
        .unwrap();

        // ReadOnly archive mem `external` with a spec-shaped entity
        // declaring an explicit cross-mem relation back to the local
        // note.
        let archive_md = b"---\ntype: spec\n---\n# Archived\n\n## Identity\n\nrefers back to [[local:note]] for the current revision.\n\n## Relationships\n\n- **REFERENCES**: [[local:note]]\n";
        let archive_path = build_archive(tmp.path(), "external", &[("archived.md", archive_md)]);

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("local", folder_dir),
                Box::new(folder_writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("external", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path)),
            ),
        ])
        .unwrap();

        // Sanity: both entities are real, both mems are mounted.
        let local_id = EntityId::new("local", "note");
        let archived_id = EntityId::new("external", "archived");
        assert!(engine.get_entity(&local_id).is_some());
        assert!(engine.get_entity(&archived_id).is_some());
        assert!(matches!(
            engine.capability("local").unwrap(),
            MountCapability::Write
        ));
        assert!(matches!(
            engine.capability("external").unwrap(),
            MountCapability::ReadOnly
        ));

        // Walk every edge in the store. For each (from, edge) pair,
        // `edge_is_from_readonly(from)` must return true iff the
        // source mount's capability is ReadOnly. The fixture's two
        // wiki-links produce one edge from each mem — both halves
        // exercise both branches of the helper.
        let mut seen_write_edge = false;
        let mut seen_readonly_edge = false;
        for from in engine.store().all_ids().cloned().collect::<Vec<_>>() {
            for _edge in engine.store().outgoing(&from) {
                let is_ro = engine.edge_is_from_readonly(&from);
                match engine.capability(from.mem()).unwrap() {
                    MountCapability::Write => {
                        assert!(
                            !is_ro,
                            "edge from write mem {} reported as ReadOnly",
                            from.mem()
                        );
                        seen_write_edge = true;
                    }
                    MountCapability::ReadOnly => {
                        assert!(
                            is_ro,
                            "edge from readonly mem {} reported as Write",
                            from.mem()
                        );
                        seen_readonly_edge = true;
                    }
                }
            }
        }
        assert!(
            seen_write_edge,
            "fixture must produce at least one edge from a write mem"
        );
        assert!(
            seen_readonly_edge,
            "fixture must produce at least one edge from a readonly mem"
        );

        // Helper also reports `false` for mems absent from the
        // router — no mount → no ReadOnly assertion can be made.
        let phantom = EntityId::new("missing-mem", "phantom");
        assert!(
            !engine.edge_is_from_readonly(&phantom),
            "absent mount must not be reported as ReadOnly"
        );
    }

    // ---- Engine::changes_since wrapper ------------------------------

    #[test]
    fn cross_mem_link_allowed_same_mem_always_true() {
        // Self-edges (from == to) bypass the cross-mem policy
        // entirely — the policy gates *cross*-mem edges only.
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        assert!(engine.cross_mem_link_allowed("specs", "specs"));
        // Even when the mem doesn't exist (not enrolled in
        // settings.cross_mem_links), same-mem returns true —
        // the engine doesn't validate mem existence here, just the
        // policy.
        assert!(engine.cross_mem_link_allowed("anywhere", "anywhere"));
    }

    #[test]
    fn cross_mem_link_allowed_absent_denies_by_default() {
        // No entry in cross_mem_links for `from_mem` → denied.
        // Default-deny is the V1 posture; operators opt in.
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        assert!(!engine.cross_mem_link_allowed("specs", "engine"));
        assert!(!engine.cross_mem_link_allowed("missing", "anywhere"));
    }

    #[test]
    fn cross_mem_link_allowed_wildcard_admits_any_target() {
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings
            .cross_mem_links
            .insert("specs".to_string(), CrossLinkValue::Wildcard);
        engine.set_settings(settings);
        assert!(engine.cross_mem_link_allowed("specs", "engine"));
        assert!(engine.cross_mem_link_allowed("specs", "macos"));
        assert!(engine.cross_mem_link_allowed("specs", "any-other"));
        // Reverse direction is independent — no policy entry for
        // engine→specs means denied.
        assert!(!engine.cross_mem_link_allowed("engine", "specs"));
    }

    #[test]
    fn cross_mem_link_allowed_allowlist_enforces_membership() {
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "specs".to_string(),
            CrossLinkValue::List(vec!["engine".to_string(), "macos".to_string()]),
        );
        engine.set_settings(settings);
        assert!(engine.cross_mem_link_allowed("specs", "engine"));
        assert!(engine.cross_mem_link_allowed("specs", "macos"));
        assert!(!engine.cross_mem_link_allowed("specs", "external"));
    }

    #[test]
    fn cross_mem_link_allowed_synthesises_from_matching_create_rule_wildcard() {
        // No explicit cross_mem_links entry, but a create rule
        // matches `from_mem` and carries default_cross_links = "*".
        // Synthesis grants permission to any target.
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings
            .mem_create_rules
            .push(crate::workspace::CreateRuleSetting {
                pattern: "exec-*".to_string(),
                schemas: vec!["default".to_string()],
                default_cross_links: Some(CrossLinkValue::Wildcard),
            });
        engine.set_settings(settings);
        // No explicit policy; synthesis grants permission for any
        // target because the rule's value is Wildcard.
        assert!(engine.cross_mem_link_allowed("exec-foo", "specs"));
        assert!(engine.cross_mem_link_allowed("exec-foo", "engine"));
        // Mem that doesn't match any rule → still denied.
        assert!(!engine.cross_mem_link_allowed("orphan", "specs"));
    }

    /// #42: synthesis matches a hierarchical mem by composing the same
    /// `<mem_path>/<name>` candidate the create-rule glob is keyed on,
    /// not the bare leaf. Before the fix, `from_mem = "project"` could
    /// never match a `memstead/*` rule (the leaf-vs-composed-path
    /// divergence), so enforcement denied a link `memstead_overview`
    /// rendered as rule-granted.
    #[test]
    fn cross_mem_link_allowed_synthesises_for_hierarchical_mem() {
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        // Mount `project` with a hierarchical branch so its `mem_path()`
        // is "memstead" and the composed candidate is "memstead/project".
        // The Folder backend handles loading; only the Mount's storage
        // feeds `mem_path()`.
        let mount = Mount {
            mem: "project".into(),
            schema: Some(pin("default")),
            storage: MountStorage::GitBranch {
                gitdir: mem_dir.join(".git"),
                branch: "memstead/project".into(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine =
            Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings
            .mem_create_rules
            .push(crate::workspace::CreateRuleSetting {
                pattern: "memstead/*".to_string(),
                schemas: vec!["default".to_string()],
                default_cross_links: Some(CrossLinkValue::List(vec!["engine".to_string()])),
            });
        engine.set_settings(settings);
        assert!(
            engine.cross_mem_link_allowed("project", "engine"),
            "synthesis must match via the composed `memstead/project` candidate"
        );
        assert!(
            !engine.cross_mem_link_allowed("project", "macos"),
            "a target outside the rule's default_cross_links is still denied"
        );
    }

    #[test]
    fn cross_mem_link_allowed_synthesises_from_matching_create_rule_list() {
        // Create rule's default_cross_links is a list — synthesis
        // grants permission to listed targets only.
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings
            .mem_create_rules
            .push(crate::workspace::CreateRuleSetting {
                pattern: "exec-*".to_string(),
                schemas: vec!["default".to_string()],
                default_cross_links: Some(CrossLinkValue::List(vec!["specs".to_string()])),
            });
        engine.set_settings(settings);
        assert!(engine.cross_mem_link_allowed("exec-foo", "specs"));
        // Target not in the synthesised list → denied.
        assert!(!engine.cross_mem_link_allowed("exec-foo", "engine"));
    }

    #[test]
    fn cross_mem_link_allowed_explicit_policy_wins_over_synthesis() {
        // Explicit cross_mem_links wildcard fires first; the
        // synthesis layer is never consulted (and would deny).
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings
            .cross_mem_links
            .insert("exec-foo".to_string(), CrossLinkValue::Wildcard);
        // The synthesis layer would deny `exec-foo → engine` (no
        // matching rule), but explicit policy returns true first.
        engine.set_settings(settings);
        assert!(engine.cross_mem_link_allowed("exec-foo", "engine"));
    }

    #[test]
    fn cross_mem_link_allowed_synthesis_unions_into_explicit_list() {
        // Explicit list = ["specs"]; create rule synthesises = ["macos"].
        // Effective allowed targets: union ({specs, macos}).
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "exec-foo".to_string(),
            CrossLinkValue::List(vec!["specs".to_string()]),
        );
        settings
            .mem_create_rules
            .push(crate::workspace::CreateRuleSetting {
                pattern: "exec-*".to_string(),
                schemas: vec!["default".to_string()],
                default_cross_links: Some(CrossLinkValue::List(vec!["macos".to_string()])),
            });
        engine.set_settings(settings);
        // Explicit allowlist contains specs → allowed.
        assert!(engine.cross_mem_link_allowed("exec-foo", "specs"));
        // Synthesis layer adds macos → allowed.
        assert!(engine.cross_mem_link_allowed("exec-foo", "macos"));
        // Neither layer allows engine → denied.
        assert!(!engine.cross_mem_link_allowed("exec-foo", "engine"));
    }

    #[test]
    fn cross_mem_link_allowed_set_settings_invalidates_compiled_rule_cache() {
        // After set_settings, a fresh policy must be reflected on the
        // next call — the lazy memo can't return stale rules.
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);

        // First settings: a rule allows exec-* → specs via synthesis.
        let mut s1 = crate::workspace::WorkspaceSettings::default();
        s1.mem_create_rules
            .push(crate::workspace::CreateRuleSetting {
                pattern: "exec-*".to_string(),
                schemas: vec!["default".to_string()],
                default_cross_links: Some(CrossLinkValue::List(vec!["specs".to_string()])),
            });
        engine.set_settings(s1);
        assert!(engine.cross_mem_link_allowed("exec-foo", "specs"));

        // Replace settings: the rule no longer carries
        // default_cross_links. Cache must invalidate so the next
        // call sees the new policy.
        let mut s2 = crate::workspace::WorkspaceSettings::default();
        s2.mem_create_rules
            .push(crate::workspace::CreateRuleSetting {
                pattern: "exec-*".to_string(),
                schemas: vec!["default".to_string()],
                default_cross_links: None,
            });
        engine.set_settings(s2);
        assert!(!engine.cross_mem_link_allowed("exec-foo", "specs"));
    }

    #[test]
    fn cross_mem_link_allowed_malformed_glob_falls_back_to_explicit_policy() {
        // Malformed pattern in a create rule causes CreateRuleSet
        // compilation to fail; the resolver logs and disables
        // synthesis, but explicit cross_mem_links still works.
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings
            .mem_create_rules
            .push(crate::workspace::CreateRuleSetting {
                pattern: "[unclosed".to_string(),
                schemas: vec!["default".to_string()],
                default_cross_links: Some(CrossLinkValue::Wildcard),
            });
        // Explicit policy still works.
        settings
            .cross_mem_links
            .insert("specs".to_string(), CrossLinkValue::Wildcard);
        engine.set_settings(settings);
        // Explicit policy: specs → engine allowed.
        assert!(engine.cross_mem_link_allowed("specs", "engine"));
        // Synthesis disabled (compilation failed); rule's would-be
        // wildcard doesn't apply.
        assert!(!engine.cross_mem_link_allowed("orphan", "anything"));
    }

    #[test]
    fn cross_mem_link_allowed_empty_list_denies_all_cross_mem_targets() {
        // [cross_mem_links] specs = [] is the explicit
        // "intentionally locked down" shape — same effect as
        // default-deny but operator-acknowledged.
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings
            .cross_mem_links
            .insert("specs".to_string(), CrossLinkValue::List(Vec::new()));
        engine.set_settings(settings);
        // Same-mem still passes — policy only gates cross-mem.
        assert!(engine.cross_mem_link_allowed("specs", "specs"));
        // Cross-mem denied to every target.
        assert!(!engine.cross_mem_link_allowed("specs", "engine"));
        assert!(!engine.cross_mem_link_allowed("specs", "anything"));
    }

    #[test]
    fn from_mounts_load_warnings_merge_into_health_summary() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let body = "---\ntype: spec\n---\n# Dup2\n\n## Identity\n\na.\n\n## Identity\n\nb.\n";
        std::fs::write(mem_dir.join("dup2.md"), body).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let summary = engine.health();
        assert!(
            summary
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::DuplicateSectionHeading { .. })),
            "health() must merge load_warnings into summary.warnings: {:?}",
            summary.warnings,
        );
    }

    #[test]
    fn workspace_root_accessor_is_none_for_engine_built_from_mounts() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        assert!(
            engine.workspace_root().is_none(),
            "from_mounts has no workspace path",
        );
        assert!(engine.load_warnings().is_empty());
    }

    #[test]
    fn health_omits_outer_repo_warning_when_workspace_root_unset() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let health = engine.health();
        assert!(
            !health
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::OuterRepoNotIgnoringMemRepo { .. })),
            "outer-repo check must skip when workspace_root is None",
        );
    }

    #[test]
    fn writable_mem_names_filters_by_capability() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("writable", mem_dir),
                Box::new(writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("sealed", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path)),
            ),
        ])
        .unwrap();

        // Only the writable mount surfaces; the archive (read-only)
        // is filtered out.
        let names = engine.writable_mem_names();
        assert_eq!(names, vec!["writable"]);
    }

    /// The default writable mem is
    /// the FIRST writable mount in declaration order — the stable seed,
    /// not the alphabetically-first name. `test` is declared first;
    /// `other` sorts ahead alphabetically but is declared second, so it
    /// is NOT the default. This is the invariant that stops a second
    /// mem from silently retargeting omitted-`mem` writes.
    #[test]
    fn default_writable_mem_is_declaration_first_not_alphabetical() {
        let tmp = TempDir::new().unwrap();
        let test_dir = tmp.path().join("test");
        let other_dir = tmp.path().join("other");
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("test", test_dir.clone()),
                Box::new(FilesystemMemWriter::new(test_dir)) as Box<dyn MemBackend>,
            ),
            (
                folder_mount("other", other_dir.clone()),
                Box::new(FilesystemMemWriter::new(other_dir)) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap();

        assert_eq!(
            engine.default_writable_mem(),
            Some("test"),
            "default must be the declaration-first writable mem, not the alphabetically-first",
        );
    }

    /// Reverse declaration order to prove the default tracks declaration
    /// order rather than a fixed name: with `other` declared first it
    /// becomes the default. Together with the test above this pins the
    /// lean as mount order, not name sort.
    #[test]
    fn default_writable_mem_follows_declaration_order() {
        let tmp = TempDir::new().unwrap();
        let other_dir = tmp.path().join("other");
        let test_dir = tmp.path().join("test");
        std::fs::create_dir_all(&other_dir).unwrap();
        std::fs::create_dir_all(&test_dir).unwrap();

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("other", other_dir.clone()),
                Box::new(FilesystemMemWriter::new(other_dir)) as Box<dyn MemBackend>,
            ),
            (
                folder_mount("test", test_dir.clone()),
                Box::new(FilesystemMemWriter::new(test_dir)) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap();

        assert_eq!(engine.default_writable_mem(), Some("other"));
    }

    /// A read-only-only workspace has no default writable mem.
    #[test]
    fn default_writable_mem_none_without_writable_mount() {
        let tmp = TempDir::new().unwrap();
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);
        let engine = Engine::from_mounts(vec![(
            archive_mount("sealed", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
        )])
        .unwrap();
        assert_eq!(engine.default_writable_mem(), None);
    }

    #[test]
    fn folder_path_for_mem_returns_path_for_folder_mounts_only() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("specs", mem_dir.clone()),
                Box::new(writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("sealed", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path)),
            ),
        ])
        .unwrap();

        // Folder mount returns its path.
        assert_eq!(engine.folder_path_for_mem("specs"), Some(mem_dir.as_path()),);
        // Archive mount returns None — caller branches on storage type.
        assert_eq!(engine.folder_path_for_mem("sealed"), None);
        // Unknown mem returns None — same as Engine::mount.
        assert_eq!(engine.folder_path_for_mem("missing"), None);
    }

    #[test]
    fn mount_accessor_returns_public_mount_shape() {
        // Build a heterogeneous engine and verify Engine::mount /
        // Engine::mounts surface the operator-facing Mount records.
        // Handlers branch on MountStorage variants through this
        // accessor (replacing full's gitdir_for / worktree_for /
        // mem_head_sha / mem_config_for direct-engine
        // accessors).
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("writable", mem_dir.clone()),
                Box::new(writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("sealed", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path.clone())),
            ),
        ])
        .unwrap();

        // Known mems: each returns a Mount whose storage variant
        // matches what the caller passed at construction.
        let folder = engine.mount("writable").expect("known mem");
        assert!(matches!(folder.storage, MountStorage::Folder { .. }));
        assert_eq!(folder.capability, MountCapability::Write);

        let archive = engine.mount("sealed").expect("known mem");
        match &archive.storage {
            MountStorage::Archive { path } => assert_eq!(path, &archive_path),
            other => panic!("expected Archive storage, got {other:?}"),
        }
        assert_eq!(archive.capability, MountCapability::ReadOnly);

        // Unknown mem — None, no panic, no error.
        assert!(engine.mount("missing").is_none());

        // Engine::mounts enumerates every mount in declaration order.
        let mounts = engine.mounts();
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].mem, "writable");
        assert_eq!(mounts[1].mem, "sealed");
    }

    #[test]
    fn mem_router_writable_set_matches_writable_mount_capability() {
        // Build an engine with one writable folder mount and one
        // read-only archive mount; the router's writable set must
        // equal the writable mount's name only.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("specs", mem_dir.clone()),
                Box::new(writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("ext", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path)),
            ),
        ])
        .unwrap();

        let router = engine.mem_router();
        assert!(router.is_writable("specs"));
        assert!(!router.is_writable("ext"));
        assert!(router.is_visible("specs"));
        assert!(router.is_visible("ext"));
        let writable: std::collections::HashSet<&String> = router.writable_mems().iter().collect();
        assert_eq!(writable.len(), 1);
        assert!(writable.contains(&"specs".to_string()));
    }

    #[test]
    fn mem_router_origin_is_explicit_toml_for_workspace_mounts() {
        // Every mount built via `from_mounts` lands as
        // `MemOrigin::ExplicitToml` — the file-adapter origin.
        // `RuntimeCreated` is reserved for `memstead_mem_create`
        // runtime registrations once that handler migrates onto
        // the unified engine.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());

        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let origin = engine
            .mem_router()
            .origin_for_mem("specs")
            .expect("known mem");
        assert_eq!(origin.kind(), "explicit");
    }

    #[test]
    fn mem_router_dir_for_writable_folder_mount_matches_storage_path() {
        // Folder-backed writable mounts surface the storage path
        // via `dir_for_mem`. Handlers consuming the router for
        // per-mem path resolution rely on this.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());

        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        assert_eq!(
            engine.mem_router().dir_for_mem("specs"),
            Some(mem_dir.as_path()),
        );
        assert_eq!(engine.mem_router().dir_for_mem("unknown"), None);
    }

    #[test]
    fn mem_router_archive_path_for_read_only_archive_mount() {
        // Read-only archive mounts register via `add_read_only` so
        // `archive_path_for_mem` resolves the archive's on-disk
        // location.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("specs", mem_dir),
                Box::new(writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("ext", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path.clone())),
            ),
        ])
        .unwrap();

        let router = engine.mem_router();
        assert_eq!(
            router.archive_path_for_mem("ext"),
            Some(archive_path.as_path()),
        );
        // Writable folder mount has no archive path.
        assert_eq!(router.archive_path_for_mem("specs"), None);
    }

    #[test]
    fn read_mem_config_via_backend_trait_folder_reads_bytes() {
        // Direct trait call against FilesystemMemWriter. Verifies
        // the backend-side primitive returns the raw bytes the
        // engine then parses.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        let body = br#"{
            "format": 1,
            "schema": "default@1.0.0",
            "writeGuidance": { "tone": "neutral" }
        }"#;
        std::fs::write(mem_dir.join(".memstead").join("config.json"), body).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir);
        let result = MemBackend::read_mem_config(&writer).unwrap();
        let bytes = result.expect("config bytes must surface");
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["schema"], "default@1.0.0");
    }

    #[test]
    fn read_mem_config_via_backend_trait_folder_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir);
        let result = MemBackend::read_mem_config(&writer).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_mem_config_via_backend_trait_archive_reads_bytes() {
        // Build an archive containing .memstead/config.json and verify
        // the ArchiveBackend impl returns its bytes.
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("seed.mem");
        let body = br#"{
            "format": 1,
            "schema": "default@1.0.0",
            "writeGuidance": { "tone": "archive" }
        }"#;
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    ".memstead/config.json",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            use std::io::Write;
            writer.write_all(body).unwrap();
            writer.finish().unwrap();
        }

        let backend = ArchiveBackend::new(archive_path);
        let result = MemBackend::read_mem_config(&backend).unwrap();
        let bytes = result.expect("config bytes must surface");
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["writeGuidance"]["tone"], "archive");
    }

    #[test]
    fn mem_config_for_returns_none_when_no_config_file_present() {
        // Folder backend without a `.memstead/config.json` file. The
        // accessor must lenient — return None, not error.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        assert!(engine.mem_config_for("specs").is_none());
    }

    #[test]
    fn mem_config_for_returns_some_when_config_file_present() {
        // Drop a valid `.memstead/config.json` into the mem dir,
        // build the engine, and assert the accessor surfaces a
        // MemConfig with the right shape (write_guidance entries
        // round-trip).
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        let config_body = r#"{
            "format": 1,
            "schema": "default@1.0.0",
            "writeGuidance": {
                "tone": "neutral",
                "voice": "active"
            }
        }"#;
        std::fs::write(mem_dir.join(".memstead").join("config.json"), config_body).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let cfg = engine
            .mem_config_for("specs")
            .expect("mem_config should load");
        assert_eq!(cfg.write_guidance.len(), 2);
        assert_eq!(
            cfg.write_guidance.get("tone").and_then(|v| v.as_str()),
            Some("neutral"),
        );
        assert_eq!(
            cfg.write_guidance.get("voice").and_then(|v| v.as_str()),
            Some("active"),
        );
    }

    #[test]
    fn mem_config_for_unknown_mem_returns_none() {
        // Lenient accessor — unknown names get None, not Err.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        assert!(engine.mem_config_for("missing").is_none());
    }

    #[test]
    fn mem_config_for_archive_mount_returns_none() {
        // Archive backends carry mem_config = None in V1 (the
        // read-from-storage path is deferred to a follow-up).
        let tmp = TempDir::new().unwrap();
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);
        let engine = Engine::from_mounts(vec![(
            archive_mount("ext", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
        )])
        .unwrap();
        assert!(engine.mem_config_for("ext").is_none());
    }

    #[test]
    fn mem_configs_named_iterates_only_mounts_with_config() {
        // Two folder mounts; one has a config file, one doesn't.
        // The iterator yields exactly the configured one — verifies
        // the filter_map shape and that the name comes from the
        // mount record (authoritative), not the config body.
        let tmp = TempDir::new().unwrap();
        let with_config = tmp.path().join("specs");
        let without_config = tmp.path().join("memos");
        std::fs::create_dir_all(with_config.join(".memstead")).unwrap();
        std::fs::create_dir_all(&without_config).unwrap();
        let config_body = r#"{
            "format": 1,
            "schema": "default@1.0.0",
            "writeGuidance": { "tone": "neutral" }
        }"#;
        std::fs::write(
            with_config.join(".memstead").join("config.json"),
            config_body,
        )
        .unwrap();

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("specs", with_config.clone()),
                Box::new(FilesystemMemWriter::new(with_config)) as Box<dyn MemBackend>,
            ),
            (
                folder_mount("memos", without_config.clone()),
                Box::new(FilesystemMemWriter::new(without_config)) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap();

        let yielded: Vec<(&str, usize)> = engine
            .mem_configs_named()
            .map(|(name, cfg)| (name, cfg.write_guidance.len()))
            .collect();
        assert_eq!(yielded, vec![("specs", 1)]);
    }

    #[test]
    fn schema_for_returns_some_for_known_mem_and_none_for_unknown() {
        // Every mount registers a schema (resolved from its pin at
        // boot). Lookup by mem name surfaces the same Arc that
        // mutations resolve internally; unknown names return None.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        assert!(engine.schema_for("specs").is_some());
        assert!(engine.schema_for("missing").is_none());
    }

    #[test]
    fn gitdir_for_unknown_mem_returns_unknown_mem() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let err = engine.gitdir_for("missing").unwrap_err();
        assert!(matches!(err, EngineError::UnknownMem(v) if v == "missing"));
    }

    #[test]
    fn gitdir_for_folder_mount_returns_no_gitdir_error() {
        // Folder mounts do not have a gitdir — full's contract surfaces
        // a mem-level error, not UnknownMem. Mirror that here.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let err = engine.gitdir_for("specs").unwrap_err();
        match err {
            EngineError::Mem(msg) => assert!(msg.contains("no resolved gitdir")),
            other => panic!("expected EngineError::Mem, got {other:?}"),
        }
    }

    #[test]
    fn worktree_for_folder_mount_returns_storage_path() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let worktree = engine.worktree_for("specs").unwrap();
        assert_eq!(worktree, mem_dir);
    }

    #[test]
    fn worktree_for_unknown_mem_returns_unknown_mem() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let err = engine.worktree_for("missing").unwrap_err();
        assert!(matches!(err, EngineError::UnknownMem(v) if v == "missing"));
    }

    #[test]
    fn worktree_for_archive_mount_returns_archive_backed_error() {
        let tmp = TempDir::new().unwrap();
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);
        let engine = Engine::from_mounts(vec![(
            archive_mount("ext", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let err = engine.worktree_for("ext").unwrap_err();
        match err {
            EngineError::Mem(msg) => assert!(msg.contains("archive-backed")),
            other => panic!("expected EngineError::Mem, got {other:?}"),
        }
    }

    #[test]
    fn mem_head_sha_for_folder_mount_is_none() {
        // Folder backend doesn't track a head; current_head() returns
        // Ok(None) at construction; mem_head_sha returns Ok(None).
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let head = engine.mem_head_sha("specs").unwrap();
        assert_eq!(head, None);
    }

    #[test]
    fn mem_head_sha_unknown_mem_returns_unknown_mem() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let err = engine.mem_head_sha("missing").unwrap_err();
        assert!(matches!(err, EngineError::UnknownMem(v) if v == "missing"));
    }

    #[test]
    fn capability_surfaces_per_mount() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("writable", mem_dir),
                Box::new(writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("read-only", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path)),
            ),
        ])
        .unwrap();

        assert_eq!(
            engine.capability("writable").unwrap(),
            MountCapability::Write
        );
        assert_eq!(
            engine.capability("read-only").unwrap(),
            MountCapability::ReadOnly
        );
        assert!(matches!(
            engine.capability("missing"),
            Err(EngineError::UnknownMem(_))
        ));
    }

    #[test]
    fn read_provenance_routes_through_backend() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());

        // Append a provenance record via the backend trait directly,
        // then read it back through the engine.
        let backend_handle: &dyn MemBackend = &writer;
        backend_handle
            .append_provenance(&Provenance::new(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
                crate::ProvenanceKind::Create,
                Some("v:e".into()),
                crate::vcs::Actor::Cli,
                None,
                Some("first".into()),
            ))
            .unwrap();

        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let records = engine.read_provenance("specs", None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, crate::ProvenanceKind::Create);
        assert_eq!(records[0].entity.as_deref(), Some("v:e"));
        assert_eq!(records[0].note.as_deref(), Some("first"));
    }

    #[test]
    fn archive_mount_returns_sealed_indirectly_through_backend_layer() {
        // The engine doesn't yet expose mutation methods, but an
        // archive backend held on a Mount with ReadOnly capability is
        // still a `&dyn MemBackend` whose write methods return
        // Sealed. This test locks the trait routing — when the engine
        // gains write methods in a later session, capability gating +
        // backend Sealed errors must agree.
        let tmp = TempDir::new().unwrap();
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);
        let backend = ArchiveBackend::new(archive_path);
        match MemBackend::write_entity(&backend, Path::new("x.md"), b"x") {
            Err(BackendError::Sealed) => {}
            other => panic!("expected Sealed, got {other:?}"),
        }
    }

    // ---- Read-side delegates ----------------------------------------
    //
    // These tests pin the surface that the MCP migration consumes
    // (stats, health, context, communities, search, list, orphans,
    // stubs, most_connected, missing_required_outgoing). They run
    // against a folder-mount engine with a small fixture of created
    // entities and one relate edge — enough to exercise both the
    // graph-query path and the cache-invalidation hooks.

    fn build_demo_engine(tmp: &TempDir) -> Engine {
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source One"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let target = engine
            .create_entity(
                empty_create_args("specs", "Target Two"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        engine
            .create_entity(
                empty_create_args("specs", "Lonely Three"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        engine
    }

    #[test]
    fn status_reports_per_engine_counts() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let stats = engine.status();
        assert_eq!(stats.entity_count, 3);
        assert_eq!(stats.edge_count, 1);
        assert_eq!(stats.mem_count, 1);
        assert_eq!(stats.types_in_use, vec!["spec".to_string()]);
        assert_eq!(stats.edge_types.get("USES"), Some(&1));
    }

    #[test]
    fn orphans_lists_unconnected_real_entities() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let orphans = engine.orphans();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].as_ref(), "specs--lonely-three");
    }

    /// #49: the orphan/community headlines can be attributed per pinned
    /// schema. Single-mem here, so one bucket — but it proves the
    /// attribution keys by `schema_of(mem)` and that the per-schema
    /// counts sum to the raw total (which a health surface keeps verbatim).
    #[test]
    fn schema_breakdowns_attribute_to_mem_pin() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);

        let orphans = engine.orphans();
        let orphans_by_schema = engine.orphans_by_schema(&orphans);
        assert_eq!(
            orphans_by_schema.values().sum::<usize>(),
            orphans.len(),
            "per-schema orphan counts must sum to the raw total"
        );
        assert_eq!(orphans_by_schema.len(), 1, "one mem ⇒ one schema bucket");
        let (schema, count) = orphans_by_schema.iter().next().unwrap();
        assert!(!schema.is_empty(), "specs mem is pinned: {schema:?}");
        assert_eq!(*count, 1);

        // communities_by_schema buckets the demo mem's clusters under the
        // same pin; with one schema, its values sum to the global count.
        let mems: Vec<String> = engine.mounts().iter().map(|m| m.mem.clone()).collect();
        let communities_by_schema = engine.communities_by_schema(&mems);
        assert_eq!(communities_by_schema.len(), 1);
        assert_eq!(
            communities_by_schema.values().sum::<usize>(),
            engine.communities().count,
        );
    }

    #[test]
    fn stubs_lists_unresolved_link_targets() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let source = engine
            .create_entity(
                empty_create_args("specs", "Holder"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Relate to a non-existent target — relate_entity creates a
        // stub for the target so the edge can land.
        engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: EntityId::new("specs", "ghost"),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let stubs = engine.stubs();
        assert!(
            stubs.iter().any(|(id, _)| id.as_ref() == "specs--ghost"),
            "expected ghost stub: {stubs:?}"
        );
    }

    #[test]
    fn most_connected_orders_by_degree() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let top = engine.most_connected(5);
        assert_eq!(top.len(), 3);
        // Source and Target each have one edge; Lonely has zero.
        let zero_degree: Vec<_> = top
            .iter()
            .filter(|c| c.total == 0)
            .map(|c| c.id.as_ref().to_string())
            .collect();
        assert_eq!(zero_degree, vec!["specs--lonely-three".to_string()]);
    }

    #[test]
    fn health_returns_per_engine_summary() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let health = engine.health();
        // `memstead_create` refuses on missing required sections, so
        // entities built through `empty_create_args` carry the
        // helper-seeded `identity` + `purpose` bodies and no longer
        // surface as missing-fields. Health remains the read-side
        // tolerance surface for legacy on-disk drift — covered by
        // the loader-tolerance tests that hand-craft pre-strict
        // markdown files.
        assert!(
            health
                .missing_fields
                .iter()
                .all(|r| r.id.as_ref() != "specs--source-one"),
            "post-strict-create fixture must not surface as missing-fields; got {:?}",
            health.missing_fields,
        );
    }

    #[test]
    fn context_carries_neighbors_and_community() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let source_id = EntityId::new("specs", "source-one");
        let ctx = engine.context(&source_id).unwrap();
        assert_eq!(ctx.entity_id, source_id);
        assert_eq!(ctx.neighbors.len(), 1);
        assert_eq!(ctx.neighbors[0].relationship, "USES");
        assert!(matches!(ctx.neighbors[0].direction, Direction::Outgoing));
    }

    #[test]
    fn communities_caches_louvain_until_invalidated() {
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        // Population reflects the current store at first call.
        let entities_before = engine.communities().entity_cluster_map.len();
        // Cache hit — repeat call returns same data.
        assert_eq!(
            engine.communities().entity_cluster_map.len(),
            entities_before
        );
        // Mutation invalidates the cache; next call re-runs against
        // the post-mutation store and includes the new entity.
        let (actor, client) = cli_actor();
        engine
            .create_entity(
                empty_create_args("specs", "Disturber"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let entities_after = engine.communities().entity_cluster_map.len();
        assert_eq!(
            entities_after,
            entities_before + 1,
            "create_entity should have invalidated community cache and added the new entity"
        );
    }

    #[test]
    fn list_filters_by_metadata_only() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let scope = SearchScope {
            entity_type: Some("spec".to_string()),
            ..Default::default()
        };
        let result = engine.list(&scope);
        // Three real spec entities created; stubs / non-spec types absent.
        assert_eq!(result.hits.len(), 3);
    }

    #[test]
    fn list_applies_schema_declared_filter_on_non_default_schema_mem() {
        // A mem pinned to `planning` (non-default schema). The
        // `decision` type declares `status` with `filterable: equality`.
        // Pre-fix, filter dispatch consulted only the built-in default
        // schema via `type_by_name`, missed `status`, silently bypassed
        // the filter, and emitted the misleading "unknown filter key"
        // warning. Post-fix, the filter is honored and no warning fires.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = Mount {
            mem: "planning".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "planning",
                semver::Version::new(0, 1, 0),
            )),
            storage: MountStorage::Folder { path: mem_dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine =
            Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
        let (actor, client) = cli_actor();

        // Two decisions with different status values; required fields
        // (decision/context/consequences sections, decided_on, deciders)
        // get placeholder defaults — the test only cares about the
        // status field's filterability.
        for (title, status) in &[("Skip Postgres", "accepted"), ("Use SQLite", "proposed")] {
            let mut metadata = indexmap::IndexMap::new();
            metadata.insert("status".to_string(), status.to_string());
            metadata.insert("deciders".to_string(), "alice".to_string());
            metadata.insert("decided_on".to_string(), "2026-05-19".to_string());
            let args = crate::engine::CreateEntityArgs {
                anchors: Vec::new(),
                mem: "planning".to_string(),
                title: title.to_string(),
                entity_type: "decision".to_string(),
                sections: indexmap::IndexMap::from_iter([
                    ("decision".to_string(), "We chose this.".to_string()),
                    ("context".to_string(), "Single-user dev.".to_string()),
                    ("consequences".to_string(), "Lose multi-writer.".to_string()),
                ]),
                metadata,
                relations: Vec::new(),
                dry_run: false,
            };
            engine
                .create_entity(args, actor, Some(&client), None)
                .unwrap();
        }

        // Filter on the schema-declared filterable field.
        let scope = SearchScope {
            entity_type: Some("decision".to_string()),
            filters: std::collections::HashMap::from([(
                "status".to_string(),
                "accepted".to_string(),
            )]),
            ..Default::default()
        };
        let result = engine.list(&scope);
        assert_eq!(
            result.hits.len(),
            1,
            "filter on schema-declared field must select only matching entities"
        );
        assert_eq!(result.hits[0].title, "Skip Postgres");
        assert!(
            result.warnings.is_empty(),
            "no warning should fire when the filter is declared by the mem's pinned schema: {:?}",
            result.warnings
        );
    }

    #[test]
    fn search_returns_results_against_built_index() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let scope = SearchScope {
            query: Some(crate::ops::Query {
                any: vec!["source".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = engine.search(&scope).expect("native search returns Ok");
        assert!(result.total >= 1, "expected ≥1 hit for source: {result:?}");
        assert!(
            result
                .hits
                .iter()
                .any(|h| h.id.as_ref() == "specs--source-one"),
            "expected source-one in hits: {result:?}"
        );
    }

    // ---- Engine::from_workspace_root (lean boot path) --------------
}
