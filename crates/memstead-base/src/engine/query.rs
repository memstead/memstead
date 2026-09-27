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

    /// Classify ONE ENTITY's data trust origin — the grain a read surface
    /// labels, and the grain a host quarantines at.
    ///
    /// [`Self::mem_origin_class`] answers for the whole mem, which is the
    /// answer for every entity of a read-only mount. It is not the answer
    /// inside a WRITABLE mem that accepts contributions: a body merged
    /// from a fork is a stranger's prose sitting in a mem classified
    /// first-party, and served undifferentiated it reads to an agent as
    /// part of the document it was told to trust. This narrows that case
    /// and nothing else:
    ///
    /// - the mem is third-party → third-party (unchanged);
    /// - the mem's proposal record does not parse → third-party for every
    ///   entity of that mem, because the record is the only evidence of
    ///   which bodies are a contributor's, and a trust label may not fail
    ///   open on a file it could not read;
    /// - the entity's slug carries marks and any marked section still
    ///   hashes to its recorded value → third-party, because that section
    ///   is still the proposer's bytes;
    /// - anything else → the mem's class.
    ///
    /// The lookup is by slug and nothing else. A rename or a re-keying
    /// retype follows the marks in the mutation's own commit
    /// ([`crate::engine::mutation::stage_proposal_marks_rename`],
    /// [`crate::engine::mutation::stage_proposal_marks_retype`]), so this
    /// path never has to guess. Inferring a moved body from matching
    /// content across slugs was tried and removed: a type whose
    /// load-bearing set is one section (many are) turns the guess into a
    /// one-line coincidence, so an owner's own entity sharing a single
    /// section with an adopted body was branded a stranger's, and a
    /// contributor could aim that by adopting a copy of it.
    ///
    /// Per marked section rather than one hash over the whole body, so an
    /// owner who rewrites one of three load-bearing sections does not
    /// launder the two the contributor wrote, and so the comparison needs
    /// neither the schema pin nor the entity type: a repin or a retype
    /// changes which sections a type calls load-bearing, and a comparison
    /// that depended on the current declaration would report a rewrite
    /// that never happened.
    ///
    /// The classification stays decided-at-write-time: the deciding act is
    /// the merge, which records the marks under the owner's own identity on
    /// the target branch, and the read only checks whether what was decided
    /// still describes the bytes. Content is read to see whether a mark
    /// still holds, never to award a class: all the content can do is DROP
    /// the third-party label by no longer matching, and only the owner can
    /// write it. The caller-declared `Identity:` trailer is never consulted
    /// — a contributor who wrote the body also chooses that string, so
    /// trust derived from it would be trust they grant themselves.
    ///
    /// The relation to [`Self::mem_origin_class`] is one-directional: this
    /// only ever tightens. Nothing a mem serves as third-party becomes
    /// first-party here, so a host that already quarantines on
    /// `third-party` quarantines the merged entity with no change on its
    /// side.
    pub fn entity_origin_class(&self, id: &crate::entity::EntityId) -> crate::render::OriginClass {
        let mem_class = self.mem_origin_class(id.mem());
        if mem_class.is_third_party() {
            return mem_class;
        }
        let Some(marks) = self.foreign_entities().get(id.mem()) else {
            return mem_class;
        };
        if marks.record_unreadable.is_some() {
            return crate::render::OriginClass::ThirdParty;
        }
        let Some(entity) = self.store.get(id).filter(|e| !e.stub) else {
            return mem_class;
        };
        let holds = |key: &String, hash: &String| -> bool {
            entity.sections.get(key).is_some_and(|content| {
                !content.trim().is_empty()
                    && &crate::preparation::entity_section_prepared_hash(key, content) == hash
            })
        };
        let foreign = marks
            .by_slug
            .get(id.path())
            .is_some_and(|sections| sections.iter().any(|(key, hash)| holds(key, hash)));
        if foreign {
            crate::render::OriginClass::ThirdParty
        } else {
            mem_class
        }
    }

    /// The per-mem foreign-section marks, read once from each writable
    /// mem's proposal record and memoised on the derived key (a merge's
    /// own [`Self::invalidate_communities`] refreshes it).
    ///
    /// Read-only mounts are skipped: every entity of one is already
    /// third-party at the mem grain, so a mark would change no answer.
    fn foreign_entities(&self) -> &crate::engine::ForeignMarks {
        if let Some((memo_key, _)) = self.foreign_entities_memo.get() {
            debug_assert_eq!(
                *memo_key,
                self.derived_key(),
                "foreign-entity memo key lags the engine — a mutation path missed \
                 invalidate_communities"
            );
        }
        &self
            .foreign_entities_memo
            .get_or_init(|| (self.derived_key(), self.compute_foreign_entities()))
            .1
    }

    fn compute_foreign_entities(&self) -> crate::engine::ForeignMarks {
        let mut out: crate::engine::ForeignMarks = HashMap::new();
        for mounted in &self.mounts {
            let mem = mounted.mount.mem.as_str();
            // By CLASS, not by writability. Every entity of a third-party
            // mem is already third-party, so a mark would change no answer
            // there; but a deployment may vouch for an installed archive as
            // first-party, and that archive carries its own record of the
            // bodies ITS owner adopted from contributors. Skipping by
            // writability served those as the vouching deployment's own.
            if self.mem_origin_class(mem).is_third_party() {
                continue;
            }
            let record = match self.read_proposal_record(mem) {
                Ok(Some(record)) => record,
                Ok(None) => continue,
                // The record is the only evidence of which bodies came from
                // a fork. Unreadable, the honest answer is that this mem's
                // entities are unconfirmable, and the conservative side of
                // unconfirmable is the stranger's: a corrupt file must not
                // relabel every adopted body as the workspace's own.
                Err(e) => {
                    out.insert(
                        mem.to_string(),
                        crate::engine::MemForeignMarks::unreadable(e.prose_render()),
                    );
                    continue;
                }
            };
            // One owner for the rules ([`crate::ops::proposal::ProposalRecord`]):
            // the write paths that have to follow a mark read them through the
            // same two methods, because four rounds of review found the same
            // defect one position over while the write path kept its own copy.
            if let Some(reason) = record.structural_defect() {
                out.insert(
                    mem.to_string(),
                    crate::engine::MemForeignMarks::unreadable(reason),
                );
                continue;
            }
            let marks = crate::engine::MemForeignMarks {
                by_slug: record.effective_marks_by_slug(),
                record_unreadable: None,
            };
            if !marks.by_slug.is_empty() {
                out.insert(mem.to_string(), marks);
            }
        }
        out
    }

    /// Why a mem's proposal record could not be read, when it could not.
    ///
    /// The origin read fails closed on such a mem: every entity of it serves
    /// `third-party`. Read surfaces pair the label with this reason for the
    /// same purpose the unreadable-anchors-sidecar condition is stated: a
    /// conservative label with no cause tells an operator that the engine
    /// calls their own mem a stranger's, and tells them nothing about why or
    /// what to repair.
    pub fn proposal_record_error(&self, mem: &str) -> Option<String> {
        self.foreign_entities()
            .get(mem)
            .and_then(|m| m.record_unreadable.clone())
    }

    /// Declare the workspace's own writing identities — a deployment fact
    /// on the same terms as [`Self::declare_mem_origin`]: set by the
    /// process that owns the engine, never persisted with a mem, never
    /// reachable over MCP.
    ///
    /// It classifies text that a party other than the workspace authored
    /// into WORKSPACE state, which is the check ledger: a row's `method`
    /// note and finding message are whatever agent the owner pointed at the
    /// ledger wrote, under an identity the launching side assigned. The
    /// assignment is the point — the identity is the deployer's declaration
    /// about who ran, not the authoring party's claim about itself, so
    /// unlike a mem-repo trailer it is a lever the authored bytes cannot
    /// move.
    ///
    /// Undeclared (the default) means every ledger identity inherits its
    /// mem's class, so an existing deployment is byte-identical.
    pub fn declare_owner_identities<I, S>(&mut self, identities: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.owner_identities
            .extend(identities.into_iter().map(Into::into));
        self.foreign_entities_memo = OnceCell::new();
    }

    /// The trust class of the prose one check row carries — its `method`
    /// note and finding message, which every surface renders verbatim.
    ///
    /// Two sources, and the STRICTER of the two wins. This deployment's own
    /// reading comes from its declared identities
    /// ([`Self::check_row_origin_class`]). An archive mount may additionally
    /// carry the class the exporting workspace sealed beside the row
    /// ([`crate::check::SealedCheck::origin`]), which is the only party that
    /// knew which identities were its own, and which matters where the mem
    /// grain cannot answer: a deployment may vouch for an installed archive
    /// as first-party, and a row the publisher's foreign checker wrote inside
    /// it is still a stranger's sentence.
    ///
    /// The sealed value may only tighten. It is publisher-supplied bytes, so
    /// honouring a sealed `first-party` would admit a caller-supplied
    /// first-party claim on a mem nobody vouched for, which is the one thing
    /// no read path may do; the closed vocabulary at validation stops an
    /// unrecognised token, not a well-formed lie.
    pub fn check_row_prose_origin(
        &self,
        mem: &str,
        entity_id: &str,
        rec: &crate::check::CheckRecord,
    ) -> crate::render::OriginClass {
        let derived = self.check_row_origin_class(mem, rec.identity.as_deref());
        if derived.is_third_party() {
            return derived;
        }
        match self.sealed_check_row_origin(mem, entity_id, rec) {
            Some(sealed) if sealed.is_third_party() => sealed,
            _ => derived,
        }
    }

    /// The class an ARCHIVE mount's sealed row carries, verbatim. `None` for
    /// every other mount and for a member sealed without the field.
    ///
    /// Callers go through [`Self::check_row_prose_origin`], which takes the
    /// stricter of this and the local reading; used raw, this is a
    /// publisher's claim.
    fn sealed_check_row_origin(
        &self,
        mem: &str,
        entity_id: &str,
        rec: &crate::check::CheckRecord,
    ) -> Option<crate::render::OriginClass> {
        if !self.is_archive_mount(mem) {
            return None;
        }
        let sealed = self.archive_checks_for(mem)?;
        let path = crate::EntityId(entity_id.to_string()).path().to_string();
        sealed
            .entities
            .get(&path)?
            .get(&crate::check::sealed_kind_of(rec))
            .and_then(|sc| sc.sealed_origin())
    }

    /// Whether the deployment declared any owner identity. Undeclared is
    /// the default and means every ledger row inherits its mem's class.
    pub fn declares_owner_identities(&self) -> bool {
        !self.owner_identities.is_empty()
    }

    /// Classify the origin of text a check row carries (its `method` note,
    /// its finding message) for a row recorded under `identity` against an
    /// entity of `mem`.
    ///
    /// Third-party when owner identities are declared and this row's
    /// identity is not one of them. An absent identity is unconfirmable,
    /// not foreign: it inherits the mem's class, the same reading the
    /// independence gate takes for a row that names nobody. With nothing
    /// declared every row inherits, which is what the engine served
    /// before.
    pub fn check_row_origin_class(
        &self,
        mem: &str,
        identity: Option<&str>,
    ) -> crate::render::OriginClass {
        let mem_class = self.mem_origin_class(mem);
        // Tighten-only, like `entity_origin_class`: this may make a row
        // third-party, never first-party. On a third-party mem the identity
        // on a row is the PUBLISHER's bytes, carried verbatim from the
        // archive, so honouring it here would admit a caller-supplied
        // first-party claim, which is the one thing no read path may do.
        if mem_class.is_third_party() || self.owner_identities.is_empty() {
            return mem_class;
        }
        match identity {
            // Absence is unconfirmable, never guessed foreign: the reading
            // the independence gate takes for a row that names nobody.
            None => mem_class,
            Some(id) if self.owner_identities.contains(id) => mem_class,
            Some(_) => crate::render::OriginClass::ThirdParty,
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
        // The foreign-mark map skips mems that are third-party at the mem
        // grain, so a declaration changes which mems it covers. Its validity
        // key is the store generation, which a declaration does not move, so
        // the memo has to be dropped here or a read taken before this call
        // would keep serving a vouched archive's adopted bodies as the
        // vouching deployment's own.
        self.foreign_entities_memo = OnceCell::new();
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
