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
    /// The anchors sidecar's parse error for `mem`, if it has one.
    ///
    /// The anchor readers below degrade a malformed sidecar to "no anchors",
    /// which keeps a read path alive but makes a corrupt file
    /// indistinguishable from an empty one. For a *reader* that is the right
    /// trade; for anything that draws a conclusion from the absence of
    /// anchors it is not — a fidelity pass would report every artifact
    /// uncovered and call it a finding, when the truth is that it could not
    /// read the file. Callers that need that distinction ask here first and
    /// refuse. `None` means the sidecar is absent (legitimately no anchors
    /// yet) or parses cleanly; the binding store draws the same distinction
    /// with its quarantine path.
    pub fn anchors_sidecar_error(&self, mem: &str) -> Option<String> {
        let mount = self.mounts.iter().find(|m| m.mount.mem == mem)?;
        // Three distinct ways to be unreadable, and only one of them is a
        // parse error. `.ok().flatten()` would collapse the first into
        // "absent", which is the very confusion this exists to prevent.
        let bytes = match mount.backend.read_anchors_sidecar() {
            // A backend error — permission denied, an IO fault. The file may
            // be perfectly well-formed; we simply could not look at it.
            Err(e) => return Some(format!("could not read the sidecar: {e}")),
            // Genuinely absent: a mem with no anchors yet. Not an error.
            Ok(None) => return None,
            Ok(Some(b)) => b,
        };
        // An empty or whitespace-only file parses as "no anchors" by a
        // deliberate tolerance in `from_bytes`. That tolerance is right for a
        // reader and wrong here: a sidecar truncated to zero by an
        // interrupted write is not a mem that never had anchors.
        if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            return Some(
                "the sidecar file is empty — an interrupted write leaves this state, and it is                  not the same as having no anchors; remove the file if the mem genuinely has none"
                    .to_string(),
            );
        }
        match crate::anchor::AnchorSidecar::from_bytes(&bytes) {
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        }
    }

    pub fn entity_anchors(&self, id: &EntityId) -> Vec<crate::anchor::Anchor> {
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == id.mem()) else {
            return Vec::new();
        };
        let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
            return Vec::new();
        };
        match crate::anchor::AnchorSidecar::from_bytes(&bytes) {
            Ok(sc) => sc.get(id.as_ref()).to_vec(),
            // Deliberate degrade-to-empty for the read path; a caller that
            // must not confuse "unreadable" with "none" checks
            // [`Self::anchors_sidecar_error`] first.
            Err(_) => Vec::new(),
        }
    }

    /// The stored anchors for `id`, each paired with its **live** resolution
    /// state when the engine could observe the source artifact this pass.
    ///
    /// Additive over [`Self::entity_anchors`]: the durable data is unchanged;
    /// `state` is the [`crate::anchor::resolve_anchor`] outcome against an
    /// observation the engine produces here. A `path`-namespace anchor
    /// (codebase / filesystem / git) is observed against the working tree at
    /// the current HEAD; an `entity`-namespace anchor is observed against the
    /// live graph by [`Self::observe_entity_anchor`]:
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
    /// For an `entity` grain the same table applies, read off the store
    /// rather than the filesystem: the entity missing (or present only as a
    /// stub) is `Orphaned`, and a hash-bearing class compares the canonical
    /// rendered markdown.
    ///
    /// `state` is `None` (unobserved — never a fabricated state) when there is
    /// no workspace root, when the grain is `url` (the engine never fetches;
    /// a url anchor's hash is the registry's prepared form of the content
    /// its observer supplied at write time), when an `entity` anchor's
    /// source declares a preparation the registry does not know (the form
    /// cannot be computed), or when an `entity` anchor's mem is **not
    /// mounted** — an unmounted mem is not a mem of deleted entities, and
    /// saying so would route a deletion proposal to prune.
    pub fn entity_anchors_resolved(&self, id: &EntityId) -> Vec<ResolvedAnchor> {
        let anchors = self.entity_anchors(id);
        let source_roots = self.anchor_source_roots(id.mem());
        let none = SuppliedObservations::new();
        anchors
            .into_iter()
            .map(|anchor| self.resolve_one(anchor, &source_roots, &none))
            .collect()
    }

    /// Observe one stored anchor and pair it with its resolution — the
    /// shared step behind every anchor read.
    fn resolve_one(
        &self,
        anchor: crate::anchor::Anchor,
        source_roots: &std::collections::BTreeMap<String, AnchorSourceJoin>,
        supplied: &SuppliedObservations,
    ) -> ResolvedAnchor {
        let observed = self.observe_anchor(&anchor, source_roots, supplied);
        let (state, observed_hash, observed_at) = match observed {
            Some(o) => (Some(o.state), o.hash, o.at),
            None => (None, None, None),
        };
        ResolvedAnchor {
            anchor,
            state,
            observed_hash,
            observed_at,
        }
    }

    /// Per-anchor observation — THE one resolution mechanism, shared by
    /// binding-backed verify (`mem_anchors_resolved`, which the ingest
    /// render/report/prune/findings paths consume), the per-entity read
    /// (`entity_anchors_resolved`), and the standalone
    /// `verify_mem_anchors` operation. Path-shaped grains
    /// (`span`/`file`/`tree`) observe under the **decision-29 candidate
    /// priority** (backlog-sweep plan 03a): the anchor's artifact path is
    /// SOURCE-relative first — when its `source` name resolves through
    /// `source_roots` to a declared pointer, the pointer-joined path is
    /// authoritative — and workspace-relative only as the fallback, tried
    /// when the source-join does not resolve. A path resolving under both
    /// joins is decided by that priority, deterministically. An anchor
    /// without a `source` (a hand-authored mem, a binding-less write)
    /// observes workspace-relative exactly as before. A `url`
    /// grain has no engine-side observation and returns `None` (the report
    /// vocabulary's `unresolvable`) — the engine never fetches; its hash is
    /// recorded from observation-supplied content at write time — as does a
    /// workspace-root-less engine. An `entity`
    /// grain is not a filesystem path but is not unobservable either — it
    /// resolves against the live graph via
    /// [`Self::observe_entity_anchor`], under the preparation its source
    /// declares (touchpoint A of [`crate::preparation`]: the registry
    /// decides the prepared form the artifact hashes as). This replaces the retired `single_path_medium_root` gate,
    /// whose single-source assumption nulled every anchor of a mem with
    /// zero or several bindings — the honest per-anchor answer supersedes
    /// the all-or-nothing mem-level one.
    ///
    /// A `url` grain is the one grain the engine cannot observe itself: it
    /// resolves from a SUPPLIED observation when the caller passed one for
    /// its artifact (`memstead verify-anchors --observations`), else from
    /// the row's recorded `last_observed` (sidecar version 2) — an
    /// observation that was made, carrying its own date so the row ages
    /// visibly — else it is unobserved (`None`), never fabricated. A
    /// supplied `absent` resolves `recheck`, not `orphaned`: an observer
    /// failing to retrieve a web resource is not the medium saying the
    /// artifact is gone, and prune must never act on it.
    fn observe_anchor(
        &self,
        anchor: &crate::anchor::Anchor,
        source_roots: &std::collections::BTreeMap<String, AnchorSourceJoin>,
        supplied: &SuppliedObservations,
    ) -> Option<Observed> {
        if anchor.grain == crate::anchor::AnchorGrain::Url {
            if let Some(obs) = supplied.get(&anchor.artifact) {
                return Some(match &obs.outcome {
                    crate::anchor::SuppliedOutcome::Absent => Observed {
                        state: crate::anchor::AnchorState::Recheck,
                        hash: None,
                        at: Some(obs.at.clone()),
                    },
                    crate::anchor::SuppliedOutcome::Present { hash } => Observed {
                        state: crate::anchor::resolve_anchor(
                            anchor,
                            &crate::anchor::ArtifactObservation::Present {
                                current_hash: Some(hash.clone()),
                            },
                        ),
                        hash: Some(hash.clone()),
                        at: Some(obs.at.clone()),
                    },
                });
            }
            return anchor.last_observed.as_ref().map(|rec| Observed {
                state: rec.state,
                hash: rec.hash.clone(),
                at: Some(rec.at.clone()),
            });
        }
        let join = anchor
            .source
            .as_deref()
            .and_then(|name| source_roots.get(name));
        // An `entity`-grain anchor points into a mem's graph, not a file
        // tree. It has always returned `None` here — "unobserved this pass" —
        // which meant it could never be drifted, never be orphaned, and always
        // blocked prune. That is the bail the S1b pilot demonstrated: a
        // deliberately stale anchor over a changed source entity went unflagged
        // while the capability matrix claimed full parity.
        if anchor.grain == crate::anchor::AnchorGrain::Entity {
            return self
                .observe_entity_anchor(anchor, join.and_then(|j| j.preparation.as_deref()))
                .map(Observed::live);
        }
        let root = self.workspace_root.as_deref()?;
        observe_path_anchor(root, anchor, join).map(Observed::live)
    }

    /// Observe an `entity`-grain anchor against the live graph — the entity-
    /// namespace counterpart of [`observe_path_anchor`], and the mechanism
    /// that makes a graph-medium binding's drift real.
    ///
    /// The artifact is an entity id. Present/absent comes from the store, so
    /// this works uniformly across backends — a git-branch mem has no
    /// working-tree file to stat, which is exactly why observation cannot go
    /// through the filesystem here. The compared form is the preparation
    /// registry's **prepared form** for `preparation` (the anchor's source's
    /// declared preparation, [`crate::preparation::entity_prepared_hash`]):
    /// the **canonical rendered markdown** when the source declares none —
    /// byte-for-byte today's form — or the load-bearing serialization under
    /// `entity-load-bearing`; hashed with the same `prepared_content_hash`
    /// the path arm uses, so an anchor's recorded hash means the same thing
    /// in both namespaces. An identifier the registry does not know cannot
    /// be prepared: the anchor is reported unobserved (`None`), never hashed
    /// under a fabricated form.
    ///
    /// A stub is treated as absent: a stub is the engine's placeholder for an
    /// unresolved reference, not the entity the anchor claims to pin. Scoring
    /// it as present would let a dangling anchor resolve clean.
    fn observe_entity_anchor(
        &self,
        anchor: &crate::anchor::Anchor,
        preparation: Option<&str>,
    ) -> Option<(crate::anchor::AnchorState, Option<String>)> {
        let id = EntityId::canonical(&anchor.artifact);

        // The mem the anchor points into must be MOUNTED before a store miss
        // can mean anything. If it is not, every entity in it is missing from
        // the store, and reporting `Orphaned` would say "the source deleted
        // these" about entities sitting untouched on disk. `None` — genuinely
        // unobserved — is the honest answer.
        //
        // This guard lives here, at the one observation site, and not at the
        // callers. An earlier fix put it in `run_verify` alone; `prune` reaches
        // anchor resolution by its own path (`mem_anchors_resolved`), so the
        // sync brief went on proposing the deletion of every destination
        // entity — a data-loss suggestion routed to the graph's only
        // maintenance writer, from a mem merely being unmounted. A guard that
        // protects one caller is not a guard on the behaviour.
        if !self.mounts.iter().any(|m| m.mount.mem == id.mem()) {
            return None;
        }

        let entity = self.store.get(&id).filter(|e| !e.stub);
        let Some(entity) = entity else {
            return Some((
                crate::anchor::resolve_anchor(anchor, &crate::anchor::ArtifactObservation::Absent),
                None,
            ));
        };
        let current_hash = if anchor.class.is_hash_bearing() {
            let type_def = self
                .schema_for(id.mem())
                .and_then(|schema| schema.get_type(&entity.entity_type));
            Some(crate::preparation::entity_prepared_hash(
                entity,
                type_def.as_deref(),
                preparation,
            )?)
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

    /// The `source name → join` map for `mem`'s bindings: the filesystem
    /// roots that anchors written in the source dialect join onto (decision
    /// 26: anchor artifact paths are source-relative first) and the
    /// preparation each source declares (touchpoint A: what the registry
    /// prepares the artifact as before hashing). Empty when the workspace
    /// has no root, the pipeline store does not load, or `mem` has no
    /// bindings — resolution then degrades to the workspace-relative dialect
    /// alone with no preparation, which is exactly the hand-authored-mem
    /// posture.
    pub(crate) fn anchor_source_roots(
        &self,
        mem: &str,
    ) -> std::collections::BTreeMap<String, AnchorSourceJoin> {
        let mut roots = std::collections::BTreeMap::new();
        let Some(root) = self.workspace_root.as_deref() else {
            return roots;
        };
        let Ok(configs) = crate::pipeline_store::load_pipeline_configs(root) else {
            return roots;
        };
        for record in configs.bindings.iter().filter(|r| r.mem == mem) {
            for source in &record.config.sources {
                roots
                    .entry(source.name.clone())
                    .or_insert_with(|| AnchorSourceJoin {
                        pointer: source.pointer.clone(),
                        preparation: source.preparation.clone(),
                        source: source.clone(),
                        deny_paths: record.config.deny_paths.clone(),
                    });
            }
        }
        roots
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
            // Source-dialect anchors (decision 26) reference the same
            // artifact under its pointer-joined workspace form — match both.
            let source_roots = self.anchor_source_roots(&mount.mount.mem);
            for (eid, anchors) in &sc.entities {
                for a in anchors {
                    // The shared decision-29 candidate rule: the join
                    // candidate exists only where the rule produces one (a
                    // climbing `../…` artifact never joins); the
                    // workspace-relative form is the `anchor_references_path`
                    // arm below.
                    let joined = a
                        .source
                        .as_deref()
                        .and_then(|name| source_roots.get(name))
                        .map(|join| {
                            artifact_candidates(&join.pointer, anchor_base_path(&a.artifact))
                        })
                        .and_then(|mut c| (c.len() > 1).then(|| c.remove(0)));
                    if anchor_references_path(a, artifact_path)
                        || joined.is_some_and(|j| {
                            path_references(
                                &j,
                                a.grain == crate::anchor::AnchorGrain::Tree,
                                artifact_path,
                            )
                        })
                    {
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
    /// engine produces — the working tree for a `path`-namespace anchor, the
    /// live graph for an `entity` one — or `None` when unobserved (never
    /// fabricated). The verify pipeline consumes it to adjudicate a mem's
    /// anchors against the source; audit/health can reuse it.
    /// Whether `mem`'s anchor sidecar records ANY anchor row — the cheap
    /// existence check [`Self::mem_anchors_resolved`] cannot serve: that
    /// walk OBSERVES every anchor (hashing live source artifacts, and for
    /// path anchors enumerating the facet's file scope) when a caller only
    /// needs to know the sidecar is non-empty. Parses the sidecar and stops
    /// there; false for an unknown mem, a backend without anchors, or an
    /// empty sidecar — the same population `mem_anchors_resolved` would
    /// report empty for, minus the observation cost.
    pub fn mem_has_anchors(&self, mem: &str) -> bool {
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == mem) else {
            return false;
        };
        let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
            return false;
        };
        let Ok(sc) = crate::anchor::AnchorSidecar::from_bytes(&bytes) else {
            return false;
        };
        sc.entities.values().any(|anchors| !anchors.is_empty())
    }

    pub fn mem_anchors_resolved(&self, mem: &str) -> Vec<(EntityId, ResolvedAnchor)> {
        self.mem_anchors_resolved_with(mem, &SuppliedObservations::new())
    }

    /// [`Self::mem_anchors_resolved`] with observer-supplied observations
    /// for the grains the engine cannot observe itself (`url`), keyed by
    /// artifact. Rows without a supplied observation resolve as they would
    /// without the map.
    pub fn mem_anchors_resolved_with(
        &self,
        mem: &str,
        supplied: &SuppliedObservations,
    ) -> Vec<(EntityId, ResolvedAnchor)> {
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == mem) else {
            return Vec::new();
        };
        let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
            return Vec::new();
        };
        let Ok(sc) = crate::anchor::AnchorSidecar::from_bytes(&bytes) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let source_roots = self.anchor_source_roots(mem);
        for (eid, anchors) in &sc.entities {
            for anchor in anchors {
                out.push((
                    EntityId(eid.clone()),
                    self.resolve_one(anchor.clone(), &source_roots, supplied),
                ));
            }
        }
        out
    }

    /// Standalone anchor verification — "do my sources still say what I
    /// recorded?" for one mem, regardless of how it was built. Walks the
    /// mem's anchor sidecar through the shared per-anchor mechanism
    /// ([`Self::observe_anchor`] via [`Self::mem_anchors_resolved`]) and
    /// classifies every anchor into the report vocabulary: `resolved`
    /// (source present, hash matches or non-hash class), `drifted`
    /// (present, hash differs, stability `stable`), `recheck` (hash
    /// differs under `unstable`, or a hash is missing on either side),
    /// `unresolvable` (source absent, or a grain/medium the mechanism
    /// does not reach — never fabricated into drift). Read-only on mem
    /// content: pure sidecar read + filesystem observation, no commit on
    /// any backend. A mem with no anchors returns an empty report.
    pub fn verify_mem_anchors(&self, mem: &str) -> Result<MemAnchorVerification, EngineError> {
        self.verify_mem_anchors_with(mem, &SuppliedObservations::new())
    }

    /// The stale axis defers to anchor state: an entity carrying at least
    /// one adjudicated hash-bearing anchor (`resolves`, `drifted` or
    /// `recheck`) reads by its anchors, not by the day threshold. `drifted`
    /// and `recheck` list the entity as their own condition whatever its
    /// age; `resolves` keeps it off the list, and when the threshold would
    /// have listed it the row moves to `anchor_fresh` so the reading names
    /// the clock that overruled the threshold. Entities with no adjudicated
    /// anchor (none at all, or only `unresolvable` / `unobserved` rows)
    /// keep the day-threshold reading, byte for byte. Derived at read time
    /// from the same verification the anchors axis runs; nothing stored.
    fn apply_anchor_clock(&self, summary: &mut crate::ops::HealthSummary, mem: Option<&str>) {
        use std::collections::HashMap;
        // Dominant adjudicated state per entity: drifted > recheck > resolves.
        fn rank(state: &str) -> Option<u8> {
            match state {
                "drifted" => Some(3),
                "recheck" => Some(2),
                "resolves" => Some(1),
                _ => None,
            }
        }
        let mut by_entity: HashMap<String, (u8, String)> = HashMap::new();
        let mut mems: Vec<String> = self.mem_names().iter().map(|s| s.to_string()).collect();
        mems.retain(|m| mem.is_none_or(|scope| scope == m));
        for m in mems {
            let Ok(report) = self.verify_mem_anchors(&m) else {
                continue;
            };
            for row in &report.anchors {
                let Some(r) = rank(&row.state) else { continue };
                let entry = by_entity
                    .entry(row.entity_id.clone())
                    .or_insert((0, String::new()));
                if r > entry.0 {
                    *entry = (r, row.state.clone());
                }
            }
        }
        if by_entity.is_empty() {
            return;
        }
        let today = crate::ops::health::days_since_epoch();
        let mut stale = std::mem::take(&mut summary.stale_entities);
        // Day-threshold rows on anchored entities leave the list: a
        // resolving anchor makes them fresh, a drifted or recheck one is
        // re-listed below under its own condition.
        let mut fresh = Vec::new();
        stale.retain(|row| match by_entity.get(row.id.0.as_str()) {
            None => true,
            Some((_, state)) => {
                if state == "resolves" {
                    fresh.push(crate::ops::StaleEntity {
                        id: row.id.clone(),
                        title: row.title.clone(),
                        days_since_modified: row.days_since_modified,
                        anchor_state: Some(state.clone()),
                    });
                }
                false
            }
        });
        let mut by_anchor: Vec<crate::ops::StaleEntity> = by_entity
            .iter()
            .filter(|(_, (_, state))| state != "resolves")
            .filter_map(|(id, (_, state))| {
                let entity = self.store.get(&crate::EntityId(id.clone()))?;
                if entity.stub || mem.is_some_and(|scope| entity.mem != scope) {
                    return None;
                }
                let days = entity
                    .metadata
                    .get("last_modified")
                    .and_then(|v| crate::ops::health::parse_iso_to_days(&v.to_frontmatter_string()))
                    .map(|d| today.saturating_sub(d))
                    .unwrap_or(0);
                Some(crate::ops::StaleEntity {
                    id: entity.id.clone(),
                    title: entity.title.clone(),
                    days_since_modified: days,
                    anchor_state: Some(state.clone()),
                })
            })
            .collect();
        stale.append(&mut by_anchor);
        stale.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        fresh.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        summary.stale_entities = stale;
        summary.anchor_fresh = fresh;
    }

    /// [`Self::verify_mem_anchors`] with observer-supplied observations
    /// (the `--observations` file of `memstead verify-anchors`). A `url`
    /// row whose artifact has a supplied observation adjudicates through
    /// the shared funnel exactly like a file row; a supplied row matching
    /// no `url` anchor of the mem is reported in `unmatched_observations`
    /// and changes nothing. The report's `recordable_observations` are the
    /// `last_observed` records the caller commits with
    /// [`Self::record_anchor_observations`] — the verification itself
    /// writes nothing.
    pub fn verify_mem_anchors_with(
        &self,
        mem: &str,
        supplied: &SuppliedObservations,
    ) -> Result<MemAnchorVerification, EngineError> {
        if !self.mem_router.is_visible(mem) {
            return Err(self.unknown_mem_error(mem));
        }
        let now = self.now_iso();
        // An unreadable sidecar is a condition, never zero rows: the readers
        // below would degrade it to "no anchors" and the report would then
        // describe a clean mem it never measured.
        if let Some(why) = self.anchors_sidecar_error(mem) {
            return Ok(MemAnchorVerification {
                mem: mem.to_string(),
                sidecar_error: Some(why),
                ..Default::default()
            });
        }
        let mut report = MemAnchorVerification {
            mem: mem.to_string(),
            unreconciled: self
                .entity_set_is_reconcilable(mem)
                .err()
                .map(str::to_string),
            ..Default::default()
        };
        let reconciled = report.unreconciled.is_none();
        let mut matched: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (eid, resolved) in self.mem_anchors_resolved_with(mem, supplied) {
            let is_url = resolved.anchor.grain == crate::anchor::AnchorGrain::Url;
            let supplied_here = is_url && supplied.contains_key(&resolved.anchor.artifact);
            if supplied_here {
                matched.insert(resolved.anchor.artifact.clone());
            }
            let observed_at = resolved.observed_at.clone();
            let unobserved_for_days = observed_at
                .as_deref()
                .and_then(|at| crate::anchor::days_between(at, &now));
            if let (true, Some(state), Some(at)) = (supplied_here, resolved.state, &observed_at) {
                report.recordable_observations.push(RecordedObservation {
                    entity: eid.to_string(),
                    artifact: resolved.anchor.artifact.clone(),
                    observation: crate::anchor::AnchorObservation {
                        at: at.clone(),
                        hash: resolved.observed_hash.clone(),
                        state,
                    },
                });
            }
            // The entity end first: a row whose holder is gone is adjudicated
            // against its artifact alone otherwise, and a matching hash then
            // reports it as `resolved` for an entity that does not exist.
            if reconciled && self.entity_is_absent(&eid) {
                report.dangling += 1;
                report.anchors.push(VerifiedAnchor {
                    entity_id: eid.to_string(),
                    artifact: resolved.anchor.artifact.clone(),
                    grain: resolved.anchor.grain.as_wire().to_string(),
                    class: resolved.anchor.class.as_wire().to_string(),
                    state: "dangling".to_string(),
                    observed_hash: resolved.observed_hash,
                    observed_at,
                    unobserved_for_days,
                    observation_supplied: supplied_here,
                });
                continue;
            }
            let state = match resolved.state {
                Some(crate::anchor::AnchorState::Resolves) => {
                    report.resolves += 1;
                    crate::anchor::AnchorState::Resolves.as_wire()
                }
                Some(crate::anchor::AnchorState::Drifted) => {
                    report.drifted += 1;
                    "drifted"
                }
                Some(crate::anchor::AnchorState::Recheck) => {
                    report.recheck += 1;
                    "recheck"
                }
                Some(crate::anchor::AnchorState::Orphaned) => {
                    report.unresolvable += 1;
                    "unresolvable"
                }
                // Split from `unresolvable` (03/05, criterion 2): the artifact
                // being GONE is a measurement; the pass not reaching the
                // artifact at all is the absence of one, and the repairs
                // differ.
                None => {
                    report.unobserved += 1;
                    "unobserved"
                }
            };
            report.anchors.push(VerifiedAnchor {
                entity_id: eid.to_string(),
                artifact: resolved.anchor.artifact.clone(),
                grain: resolved.anchor.grain.as_wire().to_string(),
                class: resolved.anchor.class.as_wire().to_string(),
                state: state.to_string(),
                observed_hash: resolved.observed_hash,
                observed_at,
                unobserved_for_days,
                observation_supplied: supplied_here,
            });
        }
        report.unmatched_observations = supplied
            .keys()
            .filter(|artifact| !matched.contains(*artifact))
            .cloned()
            .collect();
        Ok(report)
    }

    /// Mem names the engine knows about, in declaration order.
    /// Cheap; useful for callers that need to enumerate before
    /// dispatching by mem.
    pub fn mem_names(&self) -> Vec<&str> {
        self.mounts.iter().map(|m| m.mount.mem.as_str()).collect()
    }

    /// Derivation-staleness report for one mem (agent-trust plan 12):
    /// every EXPLICIT edge whose rel-type the mem's schema declares
    /// `derivation: true`, compared against its recorded baseline.
    /// Baseline differs from the target's current hash → `stale`;
    /// no baseline recorded (edge predates the declaration, or was
    /// load-derived) → `unbaselined`, distinctly — never fabricated
    /// as fresh or stale. Fresh edges are not reported. A mem whose
    /// schema declares no derivation rel-types returns the empty
    /// report; an unreadable sidecar reads as empty (every edge
    /// unbaselined) rather than an error.
    pub fn derivation_report(
        &self,
        mem: &str,
    ) -> Result<Vec<crate::ops::health::DerivationFinding>, EngineError> {
        if !self.mem_router.is_visible(mem) {
            return Err(self.unknown_mem_error(mem));
        }
        let Some(schema) = self.schemas.get(mem) else {
            return Ok(Vec::new());
        };
        let declared: std::collections::HashSet<&str> = schema
            .manifest
            .relationships
            .definitions
            .iter()
            .filter(|d| d.derivation)
            .map(|d| d.name.as_str())
            .collect();
        if declared.is_empty() {
            return Ok(Vec::new());
        }
        let sidecar = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| {
                m.backend
                    .read_entity(Path::new(crate::derivation::DERIVATION_SIDECAR_PATH))
                    .ok()
                    .flatten()
            })
            .and_then(|bytes| crate::derivation::DerivationSidecar::from_bytes(&bytes).ok())
            .unwrap_or_default();

        let mut out = Vec::new();
        let mut sources: Vec<&crate::entity::Entity> = self
            .store
            .all_entities()
            .filter(|e| !e.stub && e.id.mem() == mem)
            .collect();
        sources.sort_by(|a, b| a.id.as_ref().cmp(b.id.as_ref()));
        for entity in sources {
            for edge in self.store.outgoing(&entity.id) {
                if !declared.contains(edge.rel_type.as_str())
                    || edge.source != crate::store::EdgeSource::Explicit
                {
                    continue;
                }
                let current = self
                    .store
                    .get(&edge.target)
                    .map(|t| t.content_hash.clone())
                    .unwrap_or_default();
                match sidecar.get(entity.id.as_ref(), &edge.rel_type, edge.target.as_ref()) {
                    None => out.push(crate::ops::health::DerivationFinding {
                        source: entity.id.clone(),
                        rel_type: edge.rel_type.clone(),
                        target: edge.target.clone(),
                        state: "unbaselined".to_string(),
                        baseline: None,
                        current,
                    }),
                    Some(baseline) if baseline != current => {
                        out.push(crate::ops::health::DerivationFinding {
                            source: entity.id.clone(),
                            rel_type: edge.rel_type.clone(),
                            target: edge.target.clone(),
                            state: "stale".to_string(),
                            baseline: Some(baseline.to_string()),
                            current,
                        })
                    }
                    Some(_) => {}
                }
            }
        }
        Ok(out)
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
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
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
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
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

    /// Every configured mount, with its config when one was readable.
    ///
    /// The counterpart to [`Self::mem_configs_named`], whose contract is "mems
    /// WITH config" and which is therefore right to omit the rest. The trouble
    /// was that callers wanting to enumerate mounts reached for it anyway: a
    /// folder mount whose directory is gone returns no config rather than an
    /// error, boot stores none, and the mount became invisible to every one of
    /// them. Nine call sites shared that blind spot, and the omission was a
    /// side effect of config readability rather than anything about the mount
    /// (04/05, criteria 1 and 8).
    ///
    /// Callers that genuinely want only configured mems keep using the other
    /// one; drivers that want every mount ask for every mount.
    pub fn mounts_with_optional_config(
        &self,
    ) -> impl Iterator<Item = (&str, Option<&memstead_schema::config::MemConfig>)> {
        self.mounts
            .iter()
            .map(|m| (m.mount.mem.as_str(), m.mem_config.as_ref()))
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
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
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
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
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

    /// The quarantine roster: mems that failed their mem-level boot
    /// step and serve nothing until repaired + reloaded. Empty on a
    /// fully healthy workspace. Surfaced on overview and health.
    pub fn quarantined_mems(&self) -> &[crate::engine::QuarantinedMem] {
        &self.quarantined
    }

    /// The quarantine entry for `mem`, when it is quarantined.
    pub fn quarantine_reason(&self, mem: &str) -> Option<&crate::engine::QuarantinedMem> {
        self.quarantined.iter().find(|q| q.mount.mem == mem)
    }

    /// Whether the store's entity set for `mem` can be trusted to answer the
    /// question "does this entity still exist?".
    ///
    /// WHY this is a question and not an assumption: an anchor sidecar is
    /// keyed by entity id, and a key with no entity behind it is a dangling
    /// row (consistency-sweep 03/02). Detecting one means reading a NEGATIVE
    /// from the store, and a negative is only evidence when the store is known
    /// to hold everything the mem has. Four states break that, and each of
    /// them would otherwise turn every anchor in the mem into a false dangling
    /// report: the mem is not mounted at all, it is quarantined (serving
    /// nothing), its lazy load has not run yet (mounted, entities absent), or
    /// a file in it failed to parse, in which case an id missing from the
    /// store may be a load failure rather than a deleted entity.
    ///
    /// The last case is deliberately COARSE for non-folder mounts: load-error
    /// paths are normalized to absolute only for folder mounts, so a
    /// git-branch mem's errors carry mem-relative paths that two mems can
    /// spell identically. Attributing them by name would be a guess, and a
    /// wrong guess here fabricates dangling rows. Any load error at all
    /// therefore blocks reconciliation for a non-folder mem. The caller states
    /// the block rather than skipping silently, which is the honest direction.
    pub fn entity_set_is_reconcilable(&self, mem: &str) -> Result<(), &'static str> {
        if self.quarantine_reason(mem).is_some() {
            return Err("the mem is quarantined and serves no entities");
        }
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == mem) else {
            return Err("the mem is not mounted here");
        };
        if mount.deferred {
            return Err("the mem's lazy entity load has not run this session");
        }
        if self.load_errors.is_empty() {
            return Ok(());
        }
        match &mount.mount.storage {
            crate::workspace::MountStorage::Folder { path } => {
                if self.load_errors.iter().any(|(p, _)| p.starts_with(path)) {
                    Err(
                        "a file in this mem failed to parse, so an id missing from the store may be a load failure rather than a deleted entity",
                    )
                } else {
                    Ok(())
                }
            }
            _ => Err(
                "this workspace has files that failed to parse, and their paths cannot be attributed to one mem",
            ),
        }
    }

    /// Whether `id` names an entity this mem no longer holds. A STUB counts
    /// as missing: a stub is the placeholder an unresolved wiki-link target
    /// leaves behind, not an entity anyone wrote, so an anchor keyed to one
    /// is dangling exactly as if nothing were there.
    ///
    /// Only meaningful once [`Self::entity_set_is_reconcilable`] says yes.
    pub fn entity_is_absent(&self, id: &EntityId) -> bool {
        self.store.get(id).is_none_or(|e| e.stub)
    }

    /// Mems whose lazy entity load is still DEFERRED — on the mount
    /// roster with a resolved schema pin, but with no entities in the
    /// store yet. Read surfaces that render per-mem counts or
    /// distributions consult this: a count over a deferred mem's slice
    /// of the store is a count over nothing, and rendering it as a
    /// bare zero is the silent absence the lazy-mount contract forbids.
    /// Either trigger the load ([`Self::ensure_mems_loaded`]) or render
    /// the load state explicitly.
    pub fn deferred_mems(&self) -> Vec<&str> {
        self.mounts
            .iter()
            .filter(|m| m.deferred)
            .map(|m| m.mount.mem.as_str())
            .collect()
    }

    /// Whether `mem` is a lazy mount whose entity load has not run yet.
    pub fn mem_is_deferred(&self, mem: &str) -> bool {
        self.mounts.iter().any(|m| m.deferred && m.mount.mem == mem)
    }

    /// The typed error for a mem name that did not resolve to a
    /// serving mount: `MEM_QUARANTINED` (carrying the underlying boot
    /// failure and its repair command) when the mem is on the
    /// quarantine roster, `UNKNOWN_MEM` otherwise. Every lookup site
    /// that fails to find a mem routes here so a quarantined mem is
    /// never misreported as unknown — honest absence, with the reason.
    pub fn unknown_mem_error(&self, mem: &str) -> EngineError {
        match self.quarantine_reason(mem) {
            Some(q) => EngineError::MemQuarantined {
                mem: mem.to_string(),
                reason_code: q.reason_code.clone(),
                reason_message: q.reason_message.clone(),
            },
            None if self.recently_unmounted(mem) => EngineError::MemUnmounted {
                mem: mem.to_string(),
            },
            None => EngineError::UnknownMem(mem.to_string()),
        }
    }

    /// The workspace-level boot diagnosis a diagnostic-shell engine
    /// carries (`None` on ordinarily booted engines).
    pub fn boot_diagnosis(&self) -> Option<(&str, &str)> {
        self.boot_diagnosis
            .as_ref()
            .map(|(c, m)| (c.as_str(), m.as_str()))
    }

    /// Build a mem-less diagnostic-shell engine for a workspace whose
    /// boot failed at the WORKSPACE level (nothing loadable — e.g. an
    /// unparseable store). It serves no mems and no entities; its one
    /// job is answering overview/health with the typed boot diagnosis
    /// so a session can always ask WHY the graph is gone — the MCP
    /// server serves this instead of exiting into `-32000 Connection
    /// closed` (degrade, never disappear).
    pub fn diagnostic_shell(reason_code: String, reason_message: String) -> Engine {
        let mut engine =
            Engine::from_mounts(Vec::new()).expect("an empty mount list always constructs");
        engine.boot_diagnosis = Some((reason_code, reason_message));
        engine
    }

    /// Append boot-path quarantine entries recorded outside
    /// `from_mounts_inner` (backend-instantiation failures happen
    /// before the mount list reaches the engine constructor). Boot
    /// paths only — quarantine is a boot judgment, never a runtime
    /// mutation.
    pub fn extend_quarantine(&mut self, entries: Vec<crate::engine::QuarantinedMem>) {
        self.quarantined.extend(entries);
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
        // Generation sanity: a stored memo must match the live store
        // generation — the mutation hooks clear stale memos before any
        // read can arrive here. A mismatch would mean a mutation path
        // skipped its invalidation call (the exact silent-staleness
        // bug the generation exists to catch).
        if let Some((memo_key, _)) = self.community_memo.get() {
            debug_assert_eq!(
                *memo_key,
                self.derived_key(),
                "community memo key lags the engine — a mutation path missed invalidate_communities"
            );
        }
        &self
            .community_memo
            .get_or_init(|| (self.derived_key(), self.compute_communities()))
            .1
    }

    /// The current validity key for derived-structure memos: store
    /// generation plus schemas epoch (see [`super::DerivedKey`]).
    pub fn derived_key(&self) -> super::DerivedKey {
        super::DerivedKey {
            store_generation: self.store.generation(),
            schemas_epoch: self.schemas_epoch,
        }
    }

    fn compute_communities(&self) -> LouvainOutput {
        {
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
        }
    }

    /// Drop the cached community detection result and the grounded
    /// labelling memo — unless the store still sits at the generation
    /// each memo was computed from (flywheel W8/01). The keep case is
    /// exactly the batch-rollback path: the restored snapshot restored
    /// the generation with it, so the memo describes the live state
    /// and recomputing it would be pure waste. Every real mutation
    /// bumps the generation first, so those clears behave as before.
    /// Coupling the labelling reset here means every site that already
    /// invalidates communities (all mutation paths, drift reload,
    /// quarantine attach/detach, apply-commit) invalidates the
    /// labelling too, so a stale label can never outlive the state
    /// change that moved it.
    pub fn invalidate_communities(&mut self) {
        let key = self.derived_key();
        if !matches!(self.community_memo.get(), Some((k, _)) if *k == key) {
            self.community_memo = OnceCell::new();
        }
        if !matches!(self.labelling_memo.get(), Some((k, _)) if *k == key) {
            self.labelling_memo = OnceCell::new();
        }
    }

    /// The grounded labelling of one mem — `None` when its pinned
    /// schema declares no `relationships.labelling`. Computed on
    /// first access for every declaring mem, memoised until the next
    /// invalidation; generation-keyed like `community_memo`.
    pub fn mem_labelling(&self, mem: &str) -> Option<&crate::ops::labelling::MemLabelling> {
        // Generation sanity, same contract as `communities()`: a
        // stored memo must match the live derived key — a mismatch
        // means a mutation path missed invalidate_communities.
        if let Some((memo_key, _)) = self.labelling_memo.get() {
            debug_assert_eq!(
                *memo_key,
                self.derived_key(),
                "labelling memo key lags the engine — a mutation path missed invalidate_communities"
            );
        }
        let (_, map) = self.labelling_memo.get_or_init(|| {
            let mut out = std::collections::HashMap::new();
            for (mem_name, schema) in &self.schemas {
                if let Some(lab) = crate::ops::labelling::labelling_of(schema) {
                    out.insert(
                        mem_name.clone(),
                        crate::ops::labelling::compute_mem_labelling(
                            &self.store,
                            mem_name,
                            &lab.attack,
                        ),
                    );
                }
            }
            (self.derived_key(), out)
        });
        map.get(mem)
    }

    /// One entity's served labelling view — `None` when the entity is
    /// a stub or its mem's schema declares no labelling; serving
    /// surfaces then keep their byte-identical payloads. The shape
    /// block is present exactly when the declaration carries a
    /// `support` walk.
    pub fn computed_labelling(
        &self,
        entity: &Entity,
    ) -> Option<crate::ops::labelling::LabellingView> {
        use crate::ops::labelling::{Label, compute_shape, labelling_of};
        if entity.stub {
            return None;
        }
        let schema = self.schemas.get(entity.mem.as_str())?;
        let lab = labelling_of(schema)?;
        let mem_lab = self.mem_labelling(entity.mem.as_str())?;
        let label = *mem_lab.labels.get(entity.id.0.as_str())?;
        let defeated_by = if label == Label::Defeated {
            mem_lab.accepted_attackers_of(entity.id.0.as_str())
        } else {
            Vec::new()
        };
        let undecided_by = if label == Label::Undecided {
            mem_lab.undecided_attackers_of(entity.id.0.as_str())
        } else {
            Vec::new()
        };
        let shape = lab.support.as_ref().map(|walk| {
            let label_of = |id: &EntityId| -> Option<Label> {
                self.mem_labelling(id.mem())
                    .and_then(|ml| ml.labels.get(id.0.as_str()).copied())
            };
            compute_shape(&self.store, &entity.id, walk, &label_of)
        });
        Some(crate::ops::labelling::LabellingView {
            label,
            defeated_by,
            undecided_by,
            shape,
        })
    }

    /// The `labelling` health axis payload — per declaring mem:
    /// counts per label, the defeated list with its accepted
    /// attackers, the undecided list with its open attacker set, and
    /// the excluded cross-mem attack-edge count. One composer shared
    /// by the CLI health command and both MCP flavours.
    pub fn health_labelling_axis(&self, mem_filter: Option<&str>) -> serde_json::Value {
        use crate::ops::labelling::Label;
        let mut mems = serde_json::Map::new();
        let mut mem_names: Vec<&String> = self.schemas.keys().collect();
        mem_names.sort();
        for mem in mem_names {
            if let Some(v) = mem_filter
                && mem != v
            {
                continue;
            }
            let Some(ml) = self.mem_labelling(mem) else {
                continue;
            };
            let mut accepted = 0usize;
            let mut defeated: Vec<serde_json::Value> = Vec::new();
            let mut undecided: Vec<serde_json::Value> = Vec::new();
            for (id, label) in &ml.labels {
                match label {
                    Label::Accepted => accepted += 1,
                    Label::Defeated => defeated.push(serde_json::json!({
                        "id": id,
                        "defeated_by": ml.accepted_attackers_of(id),
                    })),
                    Label::Undecided => undecided.push(serde_json::json!({
                        "id": id,
                        "undecided_by": ml.undecided_attackers_of(id),
                    })),
                }
            }
            mems.insert(
                mem.clone(),
                serde_json::json!({
                    "counts": {
                        "accepted": accepted,
                        "defeated": defeated.len(),
                        "undecided": undecided.len(),
                    },
                    "defeated": defeated,
                    "undecided": undecided,
                    "cross_mem_edges_excluded": ml.cross_mem_edges_excluded,
                }),
            );
        }
        serde_json::Value::Object(mems)
    }

    /// Real entities with no incoming or outgoing edges — leaf-declared
    /// types exempt (their edge-less entities are terminal by
    /// construction; see [`Self::leaf_population`]).
    pub fn orphans(&self) -> Vec<EntityId> {
        crate::graph::query::find_orphans_with_schemas(&self.store, &self.schemas)
    }

    /// Count of real entities per leaf-declared type, keyed
    /// `<schema_ref>:<type>` — the visible population the orphan
    /// exemption covers.
    pub fn leaf_population(&self) -> std::collections::BTreeMap<String, usize> {
        crate::graph::query::leaf_population(&self.store, &self.schemas)
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

    /// Standing violations of declared `constraints` (the health
    /// `constraints` include) — every non-stub entity whose type
    /// declares constraints its current state violates, in
    /// deterministic `(mem, id)` order.
    pub fn constraint_findings(
        &self,
        mem_filter: Option<&str>,
    ) -> Vec<crate::ops::health::ConstraintFindingReport> {
        let check_provider = self.check_standing_provider();
        crate::ops::health::collect_constraint_findings(
            &self.store,
            mem_filter,
            &self.schemas,
            Some(&check_provider),
        )
    }

    /// The evaluated aggregate signals for one entity — `None` when
    /// the mem has no schema, the type is unknown or a stub, or the
    /// type declares no signals; serving surfaces then keep their
    /// byte-identical payloads.
    pub fn computed_signals(
        &self,
        entity: &Entity,
    ) -> Option<Vec<crate::ops::signals::ComputedSignal>> {
        if entity.stub {
            return None;
        }
        let schema = self.schemas.get(entity.mem.as_str())?;
        let td = schema.types.get(entity.entity_type.as_str())?;
        if td.signals.is_empty() {
            return None;
        }
        Some(crate::ops::signals::compute_signals(
            &self.store,
            td,
            &entity.id,
        ))
    }

    /// Every entity carrying at least one signal above `none` — the
    /// include-gated `signals` health axis.
    pub fn signal_reports(
        &self,
        mem_filter: Option<&str>,
    ) -> Vec<crate::ops::health::SignalReport> {
        crate::ops::health::collect_signal_reports(&self.store, mem_filter, &self.schemas)
    }

    /// The `signals` health axis payload — the entity roster plus
    /// per-level counts. One composer shared by the CLI health
    /// command and both MCP flavours so the axis cannot drift
    /// between surfaces.
    pub fn health_signals_axis(&self, mem_filter: Option<&str>) -> serde_json::Value {
        use memstead_schema::SignalLevel;
        let reports = self.signal_reports(mem_filter);
        let mut notice = 0usize;
        let mut warn = 0usize;
        for r in &reports {
            for s in &r.signals {
                match s.level {
                    Some(SignalLevel::Notice) => notice += 1,
                    Some(SignalLevel::Warn) => warn += 1,
                    None => {}
                }
            }
        }
        serde_json::json!({
            "entities": reports,
            "counts": { "notice": notice, "warn": warn },
        })
    }

    /// Defective section-format declarations the loaded schemas carry
    /// (lenient boot recorded them; install would have refused).
    pub fn schema_format_defects(&self) -> Vec<crate::ops::health::SchemaFormatDefect> {
        crate::ops::health::collect_schema_format_defects(&self.schemas)
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
            .ok_or_else(|| self.unknown_mem_error(mem))?;
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
                    install_hint: None,
                }
                .with_schema_install_probe(self.workspace_root())
            })?,
        };
        Ok(crate::ops::integrity::conformance_findings(
            &self.store,
            mem,
            &effective,
            &self.schemas,
        ))
    }

    /// What `mem`'s entity BODIES carry that their types do not declare
    /// (consistency-sweep 04/01): headings absorbed into the catch-all,
    /// headings repeated so that later bodies were not kept, and frontmatter
    /// keys the next write will drop.
    ///
    /// A folder mem's ledger set against its file set, per mem.
    ///
    /// **Folder mems only, and that is the point** (04/04, criterion 4). On a
    /// git-branch mem the change set is a real two-tree diff against the
    /// committed tree, so ledger-versus-files divergence is structurally
    /// impossible; emitting an always-clean version of this check there would
    /// be a surface asserting something it never had to establish, which is
    /// the failure class this bundle exists to remove. Such a mem is absent
    /// from the map rather than present and empty.
    pub fn ledger_reconciliation(
        &self,
    ) -> std::collections::BTreeMap<String, crate::filesystem::changelog::LedgerReconciliation>
    {
        let mut out = std::collections::BTreeMap::new();
        for m in &self.mounts {
            let crate::workspace::MountStorage::Folder { path } = &m.mount.storage else {
                continue;
            };
            if let Ok(r) = crate::filesystem::changelog::reconcile_ledger(path) {
                out.insert(m.mount.mem.clone(), r);
            }
        }
        out
    }

    /// Separate from [`Self::conformance_findings`] on purpose. These are
    /// observations, not violations: absorbing an undeclared heading is the
    /// catch-all working as designed, and reporting it as a finding would fail
    /// every mem that uses the feature. What a reader needs here is whether the
    /// content SURVIVES, which each observation states.
    pub fn body_observations(
        &self,
        mem: &str,
        target_schema: Option<&memstead_schema::SchemaRef>,
    ) -> Result<Vec<crate::ops::integrity::BodyObservation>, EngineError> {
        let pinned = self
            .schemas
            .get(mem)
            .ok_or_else(|| self.unknown_mem_error(mem))?;
        let effective: Arc<Schema> = match target_schema {
            None => pinned.clone(),
            Some(target) => {
                self.resolve_schema_by_ref(target)
                    .ok_or_else(|| EngineError::SchemaNotFound {
                        mem: mem.to_string(),
                        pin: target.as_display(),
                        sources: Vec::new(),
                        install_hint: None,
                    })?
            }
        };
        Ok(crate::ops::integrity::body_observations(
            &self.store,
            mem,
            &effective,
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
    /// plus cross-mem edges the workspace grant table no longer permits,
    /// projected into the `{ id, axis, code, detail }` finding shape.
    pub fn consistency_findings(
        &self,
        mem: &str,
    ) -> Result<Vec<crate::ops::integrity::IntegrityFinding>, EngineError> {
        if !self.schemas.contains_key(mem) {
            return Err(self.unknown_mem_error(mem));
        }
        let mut findings = crate::ops::integrity::consistency_findings(
            &self.store,
            mem,
            // The one grant resolver, the same one the write gate calls. Every
            // consumer of the axis reaches it through this funnel, so there is
            // no site where a second answer could be written (04/07).
            &|from, to| self.cross_mem_link_allowed(from, to),
        );
        // The provenance layer's own consistency: a sidecar the engine cannot
        // read is a finding on the mem, beside the entity-level ones. Without
        // it every anchor surface reads zero rows over this mem and calls
        // that clean (backlog, found by the evidence-engine grader 2026-09-02).
        if let Some(why) = self.anchors_sidecar_error(mem) {
            findings.push(crate::ops::integrity::IntegrityFinding {
                id: mem.to_string(),
                axis: crate::ops::integrity::IntegrityAxis::Consistency,
                code: "ANCHORS_SIDECAR_UNREADABLE".to_string(),
                detail: serde_json::json!({
                    "mem": mem,
                    "reason": why,
                    "repair": "the anchors sidecar is unreadable, so every anchor surface for this mem reports a condition instead of rows; fix or remove the sidecar file, then re-record anchors",
                }),
            });
        }
        Ok(findings)
    }

    /// Every cross-mem edge the workspace's current grant resolution does
    /// not permit, across every visible mem.
    ///
    /// A projection of [`Self::consistency_findings`] filtered to the one
    /// code, not a second scan: the revoke path and the health axis must
    /// never be able to answer differently about the same edge, and the
    /// surest way to guarantee that is for one of them to BE the other
    /// (04/07, criterion 8).
    ///
    /// Call it after the grant edit has landed and the settings have been
    /// reloaded — the answer is "what does the CURRENT policy leave
    /// unbacked", which is what an operator revoking a grant wants to know.
    ///
    /// Takes `&mut self` because it must load lazily-deferred mems first. A
    /// deferred mem's entities are not in the store, so scanning without the
    /// load would report zero ungranted edges for it and call that clean —
    /// which is precisely the silent all-clear this whole axis exists to
    /// prevent. The long-lived server engines carry lazy mounts; the CLI's
    /// fresh boot does not, so this only bites on the surface where it is
    /// hardest to notice.
    pub fn ungranted_cross_mem_edges(&mut self) -> Vec<crate::ops::integrity::IntegrityFinding> {
        self.ensure_mems_loaded(None);
        let mut mems: Vec<&String> = self.schemas.keys().collect();
        mems.sort();
        mems.into_iter()
            .filter_map(|mem| self.consistency_findings(mem).ok())
            .flatten()
            .filter(|f| f.code == "CROSS_MEM_EDGE_UNGRANTED")
            .collect()
    }

    /// The edges that went from permitted to unpermitted between two
    /// readings of [`Self::ungranted_cross_mem_edges`] — what a policy edit
    /// just orphaned, as opposed to what was already orphaned before it.
    ///
    /// A revocation that reported the whole standing set would blame this
    /// edit for every edge some earlier unrelated revocation left behind,
    /// which is a different and less useful claim (04/07, criterion 5).
    pub fn newly_ungranted(
        before: &[crate::ops::integrity::IntegrityFinding],
        after: Vec<crate::ops::integrity::IntegrityFinding>,
    ) -> Vec<crate::ops::integrity::IntegrityFinding> {
        let seen: std::collections::BTreeSet<(String, String)> = before
            .iter()
            .map(|f| (f.id.clone(), f.detail["target_id"].to_string()))
            .collect();
        after
            .into_iter()
            .filter(|f| !seen.contains(&(f.id.clone(), f.detail["target_id"].to_string())))
            .collect()
    }

    /// Engine-wide health summary across every mount.
    pub fn health(&self) -> crate::ops::HealthSummary {
        self.health_inner(None)
    }

    /// Health summary scoped to one visible mem. The scans and
    /// structural counts narrow to that mem; workspace-level facts
    /// (quarantine roster, boot diagnosis, workspace-scoped warnings)
    /// stay global — an agent scoping to one mem must still see them.
    /// A name that is quarantined or not on the visible roster refuses
    /// `UNKNOWN_MEM` — the same gate `search` applies to its `mem`
    /// filter, so "no such mem" and "healthy mem, nothing to report"
    /// can never be confused. `None` is the engine-wide sweep.
    pub fn health_scoped(
        &self,
        mem: Option<&str>,
    ) -> Result<crate::ops::HealthSummary, crate::EngineError> {
        if let Some(name) = mem
            && (self.quarantine_reason(name).is_some() || !self.mem_router.is_visible(name))
        {
            return Err(self.unknown_mem_error(name));
        }
        Ok(self.health_inner(mem))
    }

    fn health_inner(&self, mem: Option<&str>) -> crate::ops::HealthSummary {
        let fallback = engine_fallback_type();
        let mut summary =
            crate::ops::health::compute_health(&self.store, fallback.as_ref(), &self.schemas, mem);
        self.apply_anchor_clock(&mut summary, mem);
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
        // A standing property, reported here rather than on every boot: a
        // folder mem's drift cursor is its own ledger, which only the engine
        // writes, so an edit made to its files by anything else is invisible
        // and reads keep serving the pre-edit content. Silence about that is
        // the one outcome 04/04's criterion 3 forbids. Git-branch mems are
        // absent: their change set is a real two-tree diff, so the condition
        // cannot arise (criterion 4).
        for m in &self.mounts {
            if matches!(
                m.mount.storage,
                crate::workspace::MountStorage::Folder { .. }
            ) && mem.is_none_or(|scope| scope == m.mount.mem)
            {
                summary
                    .warnings
                    .push(crate::ops::WarningHint::OutOfBandEditsUndetected {
                        mem: m.mount.mem.clone(),
                    });
            }
        }
        // Quarantine roster — a boot-honesty fact, present whenever
        // non-empty, never behind an include gate. Empty (and omitted
        // from the wire) on a healthy workspace.
        summary.quarantined = self
            .quarantined
            .iter()
            .map(|q| crate::ops::QuarantinedMemReport {
                mem: q.mount.mem.clone(),
                reason_code: q.reason_code.clone(),
                reason_message: q.reason_message.clone(),
            })
            .collect();
        // Per-file load failures ride the report unconditionally, like
        // the quarantine roster — each entry's message names the remedy
        // (the merge-conflict refusal names `memstead conflicts
        // resolve`), and a remedy only a library accessor carries is a
        // capability nobody finds at the moment it is needed.
        summary.load_errors = self
            .load_errors
            .iter()
            .map(|(path, msg)| crate::ops::LoadErrorReport {
                file: path.display().to_string(),
                error: msg.clone(),
            })
            .collect();
        summary.boot_diagnosis = self
            .boot_diagnosis
            .as_ref()
            .map(|(code, message)| serde_json::json!({ "code": code, "message": message }));
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
        // Authoring-drift axis: for every pinned schema whose sealed
        // copy carries an install-provenance stamp, report a MISSING
        // authoring package (stamped path gone) or a DIVERGED one
        // (present but no longer parsed-equivalent to the seal).
        // Unstamped schemas — sealed pre-stamp, built-ins, archive
        // installs — produce no finding. Read-only on both copies.
        summary.warnings.extend(self.authoring_drift_findings());
        // Rot axis for the pins the drift axis skips: an UNSTAMPED
        // sealed package whose content no longer passes current-
        // language authoring validation gets its own low-tier hint —
        // the holding runs fine on the tolerant seal, but the package
        // (and the unlocatable authoring source it came from) is no
        // longer installable, and nothing else would say so before the
        // next install attempt. A parsing unstamped package stays
        // silent; stamped pins are the drift axis's business.
        summary.warnings.extend(self.unstamped_rot_findings());
        // Under a mem scope, mem-attributable warnings narrow to the
        // scoped mem; workspace- and request-scoped warnings return
        // `None` from `source_mem()` and stay visible regardless.
        // Mirrors the full flavour's compose filter.
        if let Some(v) = mem {
            summary
                .warnings
                .retain(|w| w.source_mem().is_none_or(|wv| wv == v));
        }
        summary
    }

    /// Compute the authoring-drift findings for every stamped pinned
    /// schema. See the call site in [`Self::health`] for the axis
    /// contract; returns an empty list when no workspace root is set
    /// (ad-hoc mount-list engines have no authoring tree to check).
    fn authoring_drift_findings(&self) -> Vec<WarningHint> {
        let Some(root) = self.workspace_root.as_deref() else {
            return Vec::new();
        };
        // Group pinning mems by (name, version) — BTreeMap for a
        // deterministic finding order.
        let mut pins: std::collections::BTreeMap<(String, String), Vec<String>> =
            std::collections::BTreeMap::new();
        for (mem, schema) in &self.schemas {
            let (name, version) = schema.id();
            pins.entry((name.to_string(), version.to_string()))
                .or_default()
                .push(mem.clone());
        }
        let mut out = Vec::new();
        for ((name, version), mut mems) in pins {
            mems.sort();
            let Some(stamped_path) = self.read_install_provenance(root, &name, &version) else {
                continue;
            };
            let schema_ref = format!("{name}@{version}");
            // A workspace-relative stamp (the portable form the installer
            // writes for in-workspace authoring dirs) resolves against
            // THIS workspace root, so the axis checks real drift on every
            // clone instead of reporting another machine's absolute path
            // as missing. Absolute stamps stay machine-pinned as-is.
            let stamped = std::path::Path::new(&stamped_path);
            let resolved: std::path::PathBuf = if stamped.is_absolute() {
                stamped.to_path_buf()
            } else {
                root.join(stamped)
            };
            let authoring = resolved.as_path();
            if !authoring.is_dir() {
                out.push(WarningHint::SchemaAuthoringSourceMissing {
                    schema_ref,
                    stamped_path,
                    mems,
                });
                continue;
            }
            let sealed = self
                .schemas
                .get(&mems[0])
                .expect("mems collected from self.schemas keys")
                .clone();
            match memstead_schema::load_schema_from_dir(authoring) {
                Err(e) => out.push(WarningHint::SchemaAuthoringSourceDiverged {
                    schema_ref,
                    stamped_path,
                    mems,
                    detail: format!("the authoring package no longer loads: {e}"),
                }),
                Ok(authored) => {
                    if schema_parsed_fingerprint(&authored) != schema_parsed_fingerprint(&sealed) {
                        out.push(WarningHint::SchemaAuthoringSourceDiverged {
                            schema_ref,
                            stamped_path,
                            mems,
                            detail: "the parsed authoring package differs from the sealed copy \
                                     the engine runs on"
                                .to_string(),
                        });
                    }
                }
            }
        }
        out
    }

    /// Compute the rot findings for every UNSTAMPED pinned schema: read
    /// the sealed package's content back (folder seal directory, or the
    /// `__MEMSTEAD:schemas/` ref via the ops bundle) and run the
    /// authoring-tier check over it. A pin with a stamp is skipped (the
    /// divergence axis owns it); a pin with no readable sealed package
    /// — built-ins resolving from the embedded catalogue — is skipped
    /// too (nothing on disk can rot). See the call site in
    /// [`Self::health`] for the axis contract.
    fn unstamped_rot_findings(&self) -> Vec<WarningHint> {
        let Some(root) = self.workspace_root.as_deref() else {
            return Vec::new();
        };
        let mut pins: std::collections::BTreeMap<(String, String), Vec<String>> =
            std::collections::BTreeMap::new();
        for (mem, schema) in &self.schemas {
            let (name, version) = schema.id();
            pins.entry((name.to_string(), version.to_string()))
                .or_default()
                .push(mem.clone());
        }
        let mut out = Vec::new();
        for ((name, version), mut mems) in pins {
            mems.sort();
            if self
                .read_install_provenance(root, &name, &version)
                .is_some()
            {
                continue; // stamped — the divergence axis checks it
            }
            let schema_ref = format!("{name}@{version}");
            // Folder seal: the sealed package is a real directory the
            // authoring loader can probe directly.
            let sealed_dir = root.join(".memstead").join("schemas").join(&schema_ref);
            let detail: Option<String> = if sealed_dir.join("schema.yaml").is_file() {
                memstead_schema::load_schema_from_dir(&sealed_dir)
                    .err()
                    .map(|e| e.to_string())
            } else if let Some((manifest, types)) = self.read_sealed_package_yamls(&name, &version)
            {
                memstead_schema::loader::check_package_reauthorable(&manifest, &types)
                    .err()
                    .map(|e| e.to_string())
            } else {
                None // no sealed copy anywhere — embedded builtin
            };
            if let Some(detail) = detail {
                out.push(WarningHint::SchemaUnstampedSourceRot {
                    schema_ref,
                    mems,
                    detail,
                });
            }
        }
        out
    }

    /// Read a sealed package's `schema.yaml` + `types/*.yaml` back from
    /// the `__MEMSTEAD:schemas/` ref, reconstructing the type-file names
    /// from the pinned parsed schema's type roster (seal-time authoring
    /// enforces stem == declared type name). `None` when the ops bundle
    /// or the package is absent — the embedded-builtin state.
    fn read_sealed_package_yamls(
        &self,
        name: &str,
        version: &str,
    ) -> Option<(String, Vec<(String, String)>)> {
        let ops = self.git_branch_ops()?;
        let root = self.workspace_root.as_deref()?;
        let gitdir = self
            .mounts
            .iter()
            .find_map(|m| match &m.mount.storage {
                crate::workspace::MountStorage::GitBranch { gitdir, .. } => Some(gitdir.clone()),
                _ => None,
            })
            .or_else(|| {
                let g = root.join("mem-repo").join(".git");
                g.is_dir().then_some(g)
            })?;
        let read = |rel: &str| -> Option<String> {
            (ops.read_schema_file)(&gitdir, name, version, rel)
                .ok()
                .flatten()
                .and_then(|bytes| String::from_utf8(bytes).ok())
        };
        let manifest = read("schema.yaml")?;
        let schema = self
            .schemas
            .values()
            .find(|s| {
                let (n, v) = s.id();
                n == name && v.to_string() == version
            })?
            .clone();
        let mut type_names: Vec<String> = schema.types.keys().cloned().collect();
        type_names.sort();
        let mut types = Vec::new();
        for t in type_names {
            if let Some(body) = read(&format!("types/{t}.yaml")) {
                types.push((t, body));
            }
        }
        Some((manifest, types))
    }

    /// Read the install-provenance stamp for a sealed schema package,
    /// checking the folder location first
    /// (`.memstead/schemas/<name>@<version>/`) and falling back to the
    /// `__MEMSTEAD:schemas/` ref via the git-branch ops bundle when
    /// wired. `None` when no stamp exists anywhere — the normal state
    /// for pre-stamp seals, built-ins, and archive installs.
    fn read_install_provenance(&self, root: &Path, name: &str, version: &str) -> Option<String> {
        let folder_stamp = root
            .join(".memstead")
            .join("schemas")
            .join(format!("{name}@{version}"))
            .join(memstead_schema::INSTALL_PROVENANCE_FILE);
        let bytes = if folder_stamp.is_file() {
            std::fs::read(&folder_stamp).ok()
        } else {
            let ops = self.git_branch_ops()?;
            let gitdir = self
                .mounts
                .iter()
                .find_map(|m| match &m.mount.storage {
                    crate::workspace::MountStorage::GitBranch { gitdir, .. } => {
                        Some(gitdir.clone())
                    }
                    _ => None,
                })
                .or_else(|| {
                    let g = root.join("mem-repo").join(".git");
                    g.is_dir().then_some(g)
                })?;
            (ops.read_schema_file)(
                &gitdir,
                name,
                version,
                memstead_schema::INSTALL_PROVENANCE_FILE,
            )
            .ok()
            .flatten()
        }?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        v.get("authoring_path")?.as_str().map(String::from)
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

        let mut edge_types: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
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
        if let Some((memo_key, _)) = self.search_indexes_memo.get() {
            debug_assert_eq!(
                *memo_key,
                self.derived_key(),
                "search memo key lags the engine — a mutation path missed invalidate_search_indexes"
            );
        }
        &self
            .search_indexes_memo
            .get_or_init(|| (self.derived_key(), build_all(&self.store, &self.schemas)))
            .1
    }

    /// Drop the cached per-mem search index map. No-op on `wasm32`
    /// where no index exists; the method stays present so mutation
    /// hooks can call it unconditionally.
    /// Incrementally maintain the search-index memo for a known
    /// touched-id set (flywheel W8/01, criterion 1): replace or remove
    /// exactly the touched documents in place and advance the memo's
    /// key, instead of dropping the whole map. Semantics:
    ///
    /// - Memo empty → nothing to maintain; the next read builds fresh.
    /// - Memo current (key matches) → no-op (rollback already landed).
    /// - Schemas epoch moved → the index FIELD SET may have changed:
    ///   the named, scoped fallback — drop the memo for a full
    ///   rebuild. Never a silent widening: this is the one case the
    ///   plan names (schema-shape change).
    /// - Otherwise: per touched id, a real non-stub entity in the
    ///   store is re-indexed (delete-then-add on the id term), and an
    ///   absent or stub entry is removed — stubs stay excluded exactly
    ///   as the bulk build excludes them. Touched mems' writers
    ///   commit; any tantivy error falls back to dropping the memo
    ///   (warn-logged), never to serving a stale index.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn maintain_search_indexes(&mut self, touched: &[crate::EntityId]) {
        let current = super::DerivedKey {
            store_generation: self.store.generation(),
            schemas_epoch: self.schemas_epoch,
        };
        let Some((memo_key, indexes)) = self.search_indexes_memo.get_mut() else {
            return;
        };
        if *memo_key == current {
            return;
        }
        if memo_key.schemas_epoch != current.schemas_epoch {
            self.search_indexes_memo = OnceCell::new();
            return;
        }
        let mut touched_mems: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for id in touched {
            let Some(idx) = indexes.get_mut(id.mem()) else {
                continue;
            };
            let result = match self.store.get(id) {
                Some(entity) if !entity.stub => idx.index_entity(entity),
                _ => idx.remove_entity(id),
            };
            if let Err(e) = result {
                tracing::warn!(
                    id = id.as_ref(),
                    error = %e,
                    "incremental index maintenance failed; dropping the memo for a full rebuild"
                );
                self.search_indexes_memo = OnceCell::new();
                return;
            }
            touched_mems.insert(id.mem());
        }
        for (mem, idx) in indexes.iter_mut() {
            if !touched_mems.contains(mem.as_str()) {
                continue;
            }
            if let Err(e) = idx.commit() {
                tracing::warn!(
                    mem = mem.as_str(),
                    error = %e,
                    "incremental index commit failed; dropping the memo for a full rebuild"
                );
                self.search_indexes_memo = OnceCell::new();
                return;
            }
        }
        *memo_key = current;
    }

    /// No-op shim on `wasm32`, mirroring `invalidate_search_indexes`
    /// so mutation paths call it unconditionally.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn maintain_search_indexes(&mut self, _touched: &[crate::EntityId]) {}

    /// Unconditionally drop the search-index memo, regardless of its
    /// generation. The forced variant exists for embedders that want
    /// to release the index's memory in a long-lived process (or force
    /// a from-scratch rebuild for verification); the generation-checked
    /// [`Self::invalidate_search_indexes`] stays the mutation-path
    /// hook.
    pub fn drop_search_indexes(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.search_indexes_memo = OnceCell::new();
        }
    }

    pub fn invalidate_search_indexes(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Same generation check as `invalidate_communities`: keep
            // the memo when the store still sits at its generation
            // (the batch-rollback case), clear otherwise.
            if let Some((memo_key, _)) = self.search_indexes_memo.get()
                && *memo_key == self.derived_key()
            {
                return;
            }
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
        // A mem filter naming a quarantined OR nonexistent mem refuses
        // typed `UNKNOWN_MEM`, matching every other mem-naming surface. A
        // success with 0 hits (the old nonexistent-mem behaviour, with a
        // missing-index warning) is indistinguishable from a true empty
        // result — the one thing a typed surface must never be.
        if let Some(mem) = scope.mem.as_deref()
            && (self.quarantine_reason(mem).is_some() || !self.mem_router.is_visible(mem))
        {
            return Err(self.unknown_mem_error(mem));
        }
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
            .ok_or_else(|| self.unknown_mem_error(mem))
    }
}

/// The base path of an anchor artifact ref — the locator suffixes a
/// medium may append (`@<commit>`, `#<span>`) stripped so the reverse
/// lookup compares paths, not versioned/located refs.
pub(crate) fn anchor_base_path(artifact: &str) -> &str {
    let cut = artifact.find(['@', '#']).unwrap_or(artifact.len());
    &artifact[..cut]
}

/// One mem's standalone anchor-verification report — the counts plus
/// the per-anchor rows, in sidecar order.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MemAnchorVerification {
    pub mem: String,
    /// The anchors sidecar could not be read: the parse or IO reason. When
    /// set, no row was counted — every count below is zero because nothing
    /// was measured, not because the mem is clean — `fully_adjudicated`
    /// reads false and the population reads unknown. One condition, one
    /// code (`ANCHORS_SIDECAR_UNREADABLE`), rendered by every surface from
    /// this field; no surface parses the sidecar on its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidecar_error: Option<String>,
    /// Source present, hash matches (or a non-hash class whose source
    /// exists). The wire name is the `AnchorState` wire form, `resolves`;
    /// `resolved` was retired on 2026-09-02 (one name per state).
    pub resolves: usize,
    /// Source present, hash differs, stability `stable` — real drift.
    pub drifted: usize,
    /// Hash differs under `unstable` stability, or a hash is missing on
    /// either side — flagged for re-examination, never called drift.
    pub recheck: usize,
    /// Source absent: a MEASURED failure. The artifact the anchor names is
    /// not there.
    pub unresolvable: usize,
    /// The anchor could not be observed at all this pass, so nothing about it
    /// was measured (consistency-sweep 03/05, criterion 2). Its own count,
    /// because `unresolvable` used to swallow it: a reader on the surface you
    /// reach WITHOUT a binding could not tell a measured failure from an
    /// absent measurement, which is the one distinction that surface exists to
    /// make.
    pub unobserved: usize,
    /// Rows whose ENTITY is gone (consistency-sweep 03/02). Its own class,
    /// counted apart from the states above: those describe the artifact end,
    /// and a vanished entity says nothing about the source. Folding it into
    /// `unresolvable` would name the wrong repair.
    pub dangling: usize,
    /// Why the entity end could not be reconciled this pass, when it could
    /// not. `dangling: 0` means "none found" only when this is `None`.
    pub unreconciled: Option<String>,
    pub anchors: Vec<VerifiedAnchor>,
    /// Supplied observations whose artifact matched no `url` anchor of the
    /// mem — reported, never silently dropped, never applied to anything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmatched_observations: Vec<String>,
    /// The `last_observed` records this verification produced from supplied
    /// observations, for the caller to commit through
    /// [`Engine::record_anchor_observations`]. Engine-internal.
    #[serde(skip)]
    pub recordable_observations: Vec<RecordedObservation>,
}

/// One observation to record onto a sidecar row (the `last_observed`
/// field), addressed by the `(entity, artifact)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedObservation {
    pub entity: String,
    pub artifact: String,
    pub observation: crate::anchor::AnchorObservation,
}

/// Observer-supplied observations keyed by artifact — the input of
/// [`Engine::verify_mem_anchors_with`].
pub type SuppliedObservations =
    std::collections::BTreeMap<String, crate::anchor::SuppliedObservation>;

/// What one observation of an anchor yielded: the resolved state, the
/// hash it saw (when any), and — for an observation that was not made
/// live this pass — when it was made.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    state: crate::anchor::AnchorState,
    hash: Option<String>,
    at: Option<String>,
}

impl Observed {
    /// A live observation made this pass (path and entity grains).
    fn live((state, hash): (crate::anchor::AnchorState, Option<String>)) -> Self {
        Observed {
            state,
            hash,
            at: None,
        }
    }
}

impl MemAnchorVerification {
    /// The population statement that must accompany this report's figures
    /// (consistency-sweep 03/05, criteria 1 and 3): what the figures were
    /// computed over, and how much of it the pass could not adjudicate.
    ///
    /// A resolution figure alone is read as health. Every W3 finding made that
    /// figure mean less than a reader assumes, and none of them made it wrong
    /// in a way anyone could see. Rendering the figure and its population as
    /// ONE unit is what stops the next such finding being invisible: a surface
    /// cannot show the number and omit the caveat, because it gets both from
    /// here or neither.
    pub fn population_statement(&self) -> String {
        // `recheck` belongs on ONE side of this sentence. A first version put
        // it in both: counted as adjudicated and then reported as not, so the
        // same rows appeared twice and the two numbers could not be reconciled
        // by a reader. A recheck row is a row whose drift could NOT be
        // asserted, which is the definition of unadjudicated.
        if let Some(why) = &self.sidecar_error {
            return format!(
                "population unknown: the anchors sidecar could not be read ({why}); no row was \
                 counted, and zero counts here are not a clean mem"
            );
        }
        let adjudicated = self.resolves + self.drifted + self.unresolvable;
        let unadjudicated = self.recheck + self.unobserved;
        let mut s = format!(
            "over {} counted row(s): {adjudicated} adjudicated, {unadjudicated} not (recheck {}, unobserved {})",
            adjudicated + unadjudicated,
            self.recheck,
            self.unobserved
        );
        if self.dangling > 0 {
            s.push_str(&format!(
                "; {} row(s) excluded, naming an entity the mem no longer holds",
                self.dangling
            ));
        }
        if let Some(why) = &self.unreconciled {
            s.push_str(&format!(
                "; the entity end was NOT reconciled ({why}), so dangling rows would not have been detected"
            ));
        }
        s
    }

    /// Whether this axis adjudicated everything it counted. False means the
    /// figures above rest on an incomplete measurement, which is not the same
    /// as a failed one.
    pub fn fully_adjudicated(&self) -> bool {
        self.sidecar_error.is_none()
            && self.recheck == 0
            && self.unobserved == 0
            && self.unreconciled.is_none()
    }
}

/// One anchor's verification row.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifiedAnchor {
    pub entity_id: String,
    pub artifact: String,
    pub grain: String,
    pub class: String,
    /// `resolves` | `drifted` | `recheck` | `unresolvable` (artifact gone) |
    /// `unobserved` (not measured this pass) | `dangling` (the entity is
    /// gone). The wire vocabulary of this field, which is NOT the engine's
    /// `AnchorState` enum: that has four variants describing the artifact
    /// end, and the last two here are conditions beside them.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_hash: Option<String>,
    /// When the observation this row's state rests on was made — present
    /// only for a row resolved from a supplied or recorded observation (a
    /// `url` row); live observations carry none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    /// Whole days between `observed_at` and this run — how long the row has
    /// gone unobserved. `Some(0)` for an observation made today.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unobserved_for_days: Option<u64>,
    /// Whether this row's state came from an observation supplied to this
    /// run (as opposed to a live or a previously recorded one).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub observation_supplied: bool,
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
    /// the source artifact this pass: a `url` grain, no workspace root, an
    /// ambiguous or absent path medium, or an `entity` grain whose mem is not
    /// mounted. That last case is load-bearing — an unmounted mem is not a mem
    /// of deleted entities, so it must read as unobserved rather than
    /// `Orphaned`, which prune would act on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::anchor::AnchorState>,
    /// The prepared-content hash the observation computed this pass —
    /// present only for a hash-bearing (`anchored` / `derived`) anchor whose
    /// artifact could be read: a `file` / `span` anchor resolving to a
    /// readable file, or an `entity` anchor whose mem is mounted (hashed over
    /// the canonical rendered markdown, so the value means the same thing in
    /// both namespaces). The verify pass's backfill leg records it onto a
    /// hash-less anchor. Engine-internal
    /// observation detail, deliberately not serialized: the wire shape stays
    /// the stored anchor plus `state`.
    #[serde(skip)]
    pub observed_hash: Option<String>,
    /// When the observation `state` rests on was made, for a row whose
    /// observation is supplied or recorded rather than live (a `url` row).
    /// Additive: absent on every live-observed row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

/// What an anchor's `source` name resolves to in its mem's bindings: the
/// declared pointer (the filesystem root a source-dialect artifact path
/// joins onto, decision 26) and the declared preparation (what the
/// preparation registry prepares the artifact as before hashing —
/// touchpoint A of [`crate::preparation`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnchorSourceJoin {
    /// The source's declared `pointer`.
    pub(crate) pointer: String,
    /// The source's declared `preparation`, if any.
    pub(crate) preparation: Option<String>,
    /// The declaring source itself — its scope is what a `tree` anchor's
    /// prepared form enumerates under a code-map preparation.
    pub(crate) source: crate::pipeline::Source,
    /// The binding's `deny_paths`, applied on top of the source scope.
    pub(crate) deny_paths: Vec<String>,
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
/// hash. The prepared form is the registry's rule for the anchor's
/// source's preparation ([`crate::preparation::path_prepared_hash`]): a
/// `span` anchor hashes its whole containing file (the span locator selects
/// within it; the file is the hashed unit), except under a **delivery
/// preparation**, where a `<path>#<key>` span names one delivery unit (the
/// unit's own text is the hashed unit, and a key the file no longer yields
/// is an absent artifact); under a **code-map** preparation a file or span
/// hashes the interface digest, and a `tree` hashes the code map of every
/// scoped file under it. A `tree` under any other preparation hashes the
/// plain per-file prepared-content map of its scoped files, so tree anchors
/// adjudicate deterministically like file anchors. A `tree` with no
/// resolvable source-join (the enumeration scope is undefined without one),
/// a partial enumeration, and a read failure observe no hash — those
/// resolve `recheck`, never a fabricated `drifted`. Non-hash classes
/// (`authored` / `informed-by`) skip the read entirely, so an anchor-less or
/// hash-free mem pays no observation cost.
fn observe_path_anchor(
    root: &Path,
    anchor: &crate::anchor::Anchor,
    join: Option<&AnchorSourceJoin>,
) -> Option<(crate::anchor::AnchorState, Option<String>)> {
    use crate::anchor::AnchorGrain;
    match anchor.grain {
        AnchorGrain::Span | AnchorGrain::File | AnchorGrain::Tree => {}
        AnchorGrain::Url | AnchorGrain::Entity => return None,
    }
    let source_pointer = join.map(|j| j.pointer.as_str());
    let preparation = join.and_then(|j| j.preparation.as_deref());
    let base = anchor_base_path(&anchor.artifact);
    // Decision 29: the source-join is authoritative — an artifact path is
    // source-relative first (joined onto the declaring source's pointer,
    // which may deliberately leave the workspace root for out-of-root
    // pointers); the workspace-relative form is tried only when the
    // source-join does not resolve. The candidate set is the shared
    // [`artifact_candidates`] rule, so resolution, the write gate, and the
    // population matcher read one artifact the same way.
    let path = {
        let candidates = artifact_candidates(source_pointer.unwrap_or(""), base);
        candidates
            .iter()
            .map(|c| root.join(c))
            .find(|p| p.exists())
            .unwrap_or_else(|| root.join(base))
    };
    if !path.exists() {
        return Some((
            crate::anchor::resolve_anchor(anchor, &crate::anchor::ArtifactObservation::Absent),
            None,
        ));
    }
    let current_hash = if !anchor.class.is_hash_bearing() {
        None
    } else if matches!(anchor.grain, AnchorGrain::File | AnchorGrain::Span) && path.is_file() {
        match std::fs::read(&path).ok().map(|bytes| {
            crate::preparation::path_prepared_hash(
                preparation,
                &anchor.artifact,
                anchor.grain,
                &bytes,
            )
        }) {
            Some(crate::preparation::PathPrepared::Hash(h)) => Some(h),
            Some(crate::preparation::PathPrepared::UnitAbsent) => {
                return Some((
                    crate::anchor::resolve_anchor(
                        anchor,
                        &crate::anchor::ArtifactObservation::Absent,
                    ),
                    None,
                ));
            }
            Some(crate::preparation::PathPrepared::NoHash) | None => None,
        }
    } else if anchor.grain == AnchorGrain::Tree
        && path.is_dir()
        && let Some(join) = join
    {
        // The tree's prepared form: a digest over every scoped file under
        // the tree, by the declaring source's own scope and the binding's
        // deny paths — the code map under a code-map preparation, the plain
        // per-file prepared-content map otherwise, so a tree anchor
        // adjudicates deterministically instead of resting in `recheck`
        // forever. The path the anchor names is workspace-relative or
        // source-relative; the enumeration is workspace-relative, so compare
        // the resolved absolute paths. A PARTIAL enumeration (malformed or
        // retired-dialect scope pattern) observes no hash — a digest over a
        // set that is not the population would silently change a stored
        // tree-anchor hash; no-hash resolves `recheck`, the same posture as
        // a failed read.
        let enumeration = crate::ingest::cursor::enumerate_facet_files_reported(
            &join.source,
            &join.deny_paths,
            root,
        );
        if enumeration.is_partial() {
            None
        } else if preparation == Some(crate::preparation::CODE_MAP) {
            let files: Vec<(String, String)> = enumeration
                .files
                .into_iter()
                .filter(|f| root.join(f).starts_with(&path))
                .filter_map(|f| {
                    std::fs::read(root.join(&f))
                        .ok()
                        .map(|bytes| (f, String::from_utf8_lossy(&bytes).into_owned()))
                })
                .collect();
            Some(crate::anchor::prepared_content_hash(
                crate::preparation::code_map_tree_digest(&files).as_bytes(),
            ))
        } else {
            let files: Vec<(String, Vec<u8>)> = enumeration
                .files
                .into_iter()
                .filter(|f| root.join(f).starts_with(&path))
                .filter_map(|f| std::fs::read(root.join(&f)).ok().map(|bytes| (f, bytes)))
                .collect();
            Some(crate::anchor::prepared_content_hash(
                crate::preparation::plain_tree_digest(&files).as_bytes(),
            ))
        }
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

/// Deterministic fingerprint of a PARSED schema for the
/// authoring-drift equivalence check. Compares semantic content, never
/// raw bytes: YAML comments (the CLI-injected editor-header lines) and
/// whitespace vanish at parse time, and `Schema.types` — a `HashMap`
/// with nondeterministic iteration order — is rendered sorted by type
/// name so two loads of equivalent packages always fingerprint alike.
fn schema_parsed_fingerprint(schema: &memstead_schema::Schema) -> String {
    let mut keys: Vec<&String> = schema.types.keys().collect();
    keys.sort();
    let types: Vec<String> = keys
        .iter()
        .map(|k| format!("{k}={:?}", schema.types[k.as_str()]))
        .collect();
    format!(
        "{:?}|{}|{}",
        schema.manifest,
        schema.version,
        types.join(";")
    )
}

fn anchor_references_path(anchor: &crate::anchor::Anchor, path: &str) -> bool {
    let base = anchor_base_path(&anchor.artifact);
    path_references(base, anchor.grain == crate::anchor::AnchorGrain::Tree, path)
}

/// Whether `base` (a file path, or a tree root when `is_tree`) references
/// `path` — exact match, or containment for a tree.
fn path_references(base: &str, is_tree: bool, path: &str) -> bool {
    if base == path {
        return true;
    }
    if is_tree {
        let prefix = base.strip_suffix('/').unwrap_or(base);
        return path.starts_with(&format!("{prefix}/"));
    }
    false
}

/// Join a source pointer and a source-relative artifact path into the
/// pointer-joined (workspace-relative) form — the decision-26 dialect
/// bridge. Plain string concatenation with a separator: the pointer is
/// workspace-relative (and may climb out via `..`), the artifact is
/// source-relative; no canonicalization here, the filesystem resolves it.
pub(crate) fn join_pointer(pointer: &str, base: &str) -> String {
    let pointer = pointer.trim_end_matches('/');
    if pointer.is_empty() || pointer == "." {
        base.to_string()
    } else {
        format!("{pointer}/{base}")
    }
}

/// The candidate workspace-relative forms an anchor artifact could denote
/// under its declaring source's pointer, in the ratified priority (bundle
/// decision 29): the source-join first, the workspace-relative form as the
/// fallback. **The single implementation of that rule** — resolution, the
/// write-time gate, and the population scope matcher all construct their
/// candidate set here, so one artifact cannot read differently across the
/// three (each site then applies its own predicate: existence for
/// resolution and the gate, glob membership for the matcher).
///
/// One lexical clarification rides with the rule: an artifact that climbs
/// (`../…`) never joins. An artifact of a source never climbs OUT of that
/// source, so such a path is already the workspace-relative form; joining
/// anyway fabricates `<ptr>/../…`, which the filesystem then resolves into a
/// sibling tree — a false resolution for an existence test and a false
/// in-scope for a `**` glob. An artifact already carrying the pointer prefix
/// is NOT suppressed: on a self-nested layout both readings exist and the
/// decision's priority — source-join wins — settles it deterministically.
pub(crate) fn artifact_candidates(pointer: &str, base: &str) -> Vec<String> {
    let pointer = pointer.trim_end_matches('/');
    if pointer.is_empty() || pointer == "." {
        return vec![base.to_string()];
    }
    if base.starts_with("../") || base == ".." {
        return vec![base.to_string()];
    }
    vec![format!("{pointer}/{base}"), base.to_string()]
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

    /// The shared decision-29 candidate rule: source-join first,
    /// workspace-relative fallback; a pointer-less (or `.`) source has one
    /// reading; a climbing `../…` artifact never joins (the fabricated
    /// `<ptr>/../…` would resolve into a sibling tree).
    #[test]
    fn artifact_candidates_follow_decision_29_priority() {
        use super::artifact_candidates;
        assert_eq!(artifact_candidates("", "a/b.rs"), vec!["a/b.rs"]);
        assert_eq!(artifact_candidates(".", "a/b.rs"), vec!["a/b.rs"]);
        assert_eq!(artifact_candidates("./", "a/b.rs"), vec!["a/b.rs"]);
        assert_eq!(
            artifact_candidates("sub", "a/b.rs"),
            vec!["sub/a/b.rs", "a/b.rs"]
        );
        assert_eq!(
            artifact_candidates("sub/", "a/b.rs"),
            vec!["sub/a/b.rs", "a/b.rs"]
        );
        // Self-nesting is settled by priority, not suppression: the artifact
        // already carrying the pointer prefix still offers the join first.
        assert_eq!(
            artifact_candidates("sub", "sub/x.rs"),
            vec!["sub/sub/x.rs", "sub/x.rs"]
        );
        // A climbing artifact is already the workspace-relative form.
        assert_eq!(
            artifact_candidates("sub", "../dev/x.md"),
            vec!["../dev/x.md"]
        );
        assert_eq!(
            artifact_candidates("../dev", "../dev/x.md"),
            vec!["../dev/x.md"]
        );
        assert_eq!(artifact_candidates("sub", ".."), vec![".."]);
    }

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
        // Newest default generation so the clean-boot assertion below
        // isn't tripped by the SCHEMA_GENERATIONS_BEHIND hint.
        let mut mount = folder_mount("specs", mem_dir);
        mount.schema = Some("default@1.3.0".parse().unwrap());
        let engine =
            Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
        assert!(
            engine.workspace_root().is_none(),
            "from_mounts has no workspace path",
        );
        // An entity-less mount reports itself empty; nothing else.
        assert!(
            engine
                .load_warnings()
                .iter()
                .all(|w| w.code() == "MOUNT_UNBACKED"),
            "{:?}",
            engine.load_warnings()
        );
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

    /// Generation-keyed memos (flywheel W8/01). Four claims: repeated
    /// reads serve the memo; a REFUSED engine batch leaves the memo
    /// untouched (the refusal path calls no hook — pinned so it stays
    /// cheap); a memo computed from an INTERIM (mid-batch) state never
    /// survives a rollback as fresh — the store-carried generation is
    /// what lets the invalidation hook adjudicate that correctly; and
    /// a real mutation invalidates, with the recomputation identical
    /// to a from-scratch detection (the criterion-3 identity oracle
    /// for the mechanism).
    #[test]
    fn generation_keyed_memos_survive_rollback_and_track_mutations() {
        use indexmap::IndexMap;

        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);

        // 1. Repeated reads: cell filled once, generation stable.
        let _ = engine.communities();
        let memo_gen_before = engine.community_memo.get().expect("memo filled").0;
        let _ = engine.communities();
        assert_eq!(
            engine.community_memo.get().expect("still filled").0,
            memo_gen_before,
            "repeated reads must serve the memo"
        );

        // 2. A REFUSED engine batch rolls the store back and leaves
        // the memo untouched — refusal stays recompute-free.
        let bare = |id: crate::EntityId| crate::engine::UpdateEntityArgs {
            anchors: Vec::new(),
            anchors_unset: Vec::new(),
            id,
            expected_hash: None,
            sections: IndexMap::new(),
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            sections_unset: Vec::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            declare_relations: Vec::new(),
            dry_run: false,
            relations_unset: Vec::new(),
        };
        let mut real = bare(crate::EntityId::new("specs", "source-one"));
        real.append_sections
            .insert("identity".to_string(), "appended line".to_string());
        let missing = bare(crate::EntityId::new("specs", "does-not-exist"));
        let (actor, client) = cli_actor();
        let result = engine
            .batch_update(
                vec![(real, None), (missing, None)],
                actor,
                Some(&client),
                false,
            )
            .expect("refused batch returns a report-all envelope");
        assert!(
            !result.applied,
            "the missing target refuses the whole batch"
        );
        assert_eq!(
            engine.community_memo.get().map(|m| m.0),
            Some(memo_gen_before),
            "a refused batch must leave the pre-batch memo standing"
        );
        assert_eq!(
            engine.store().generation(),
            memo_gen_before.store_generation,
            "rollback restored the store to the memo's generation"
        );

        // 3. The dangerous direction: a memo computed from an INTERIM
        // state (simulated batch staging) must not survive the
        // rollback as fresh. The store-carried generation is what the
        // invalidation hook adjudicates with.
        let snapshot = engine.store.clone();
        let interim_id = crate::EntityId::new("specs", "interim-only");
        let mut interim = engine
            .store
            .get(&crate::EntityId::new("specs", "source-one"))
            .expect("demo entity present")
            .clone();
        interim.id = interim_id.clone();
        interim.title = "Interim Only".to_string();
        engine.store.upsert(interim_id, interim);
        engine.invalidate_communities();
        let _ = engine.communities();
        let interim_gen = engine.community_memo.get().expect("interim memo").0;
        assert_ne!(interim_gen, memo_gen_before);
        engine.store = snapshot; // rollback, generation restored with it
        engine.invalidate_communities();
        engine.invalidate_search_indexes();
        if let Some((g, _)) = engine.community_memo.get() {
            assert_ne!(
                *g, interim_gen,
                "a rolled-back interim state must never be served as fresh"
            );
        }
        assert!(
            !engine
                .communities()
                .entity_cluster_map
                .keys()
                .any(|id| id.contains("interim-only")),
            "the partition served after rollback reflects the restored store, not the interim one"
        );

        // 4. A real mutation invalidates; the recomputation equals a
        // from-scratch detection and sees the new entity.
        let (actor, client) = cli_actor();
        engine
            .create_entity(
                empty_create_args("specs", "Fourth Entity"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(
            engine
                .communities()
                .entity_cluster_map
                .keys()
                .any(|id| id.contains("fourth-entity")),
            "recomputed partition sees the new entity"
        );
        let fresh = {
            let schema = engine
                .schemas
                .iter()
                .min_by(|a, b| a.0.cmp(b.0))
                .map(|(_, s)| s.clone())
                .expect("demo engine has a schema");
            let schema_for_weights = schema.clone();
            crate::graph::community::detect_communities(
                engine.store(),
                schema.manifest.community.resolution,
                schema.manifest.community.seed,
                move |rel_type| {
                    schema_for_weights
                        .manifest
                        .relationships
                        .definitions
                        .iter()
                        .find(|d| d.name == rel_type)
                        .map(|d| d.default_weight as f64)
                        .unwrap_or(1.0)
                },
            )
        };
        assert_eq!(
            engine.communities().entity_cluster_map,
            fresh.entity_cluster_map,
            "memo must equal a from-scratch detection over the current store"
        );
    }

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
                    dry_run: false,
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
                    dry_run: false,
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
