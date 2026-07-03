//! Mem-lifecycle orchestrator — full home for the multi-mem create
//! and delete pipelines. The matcher primitives
//! ([`memstead_base::CreateRuleSet`], [`memstead_base::DeleteRuleSet`],
//! [`memstead_base::MatcherSet`]) stay in lean because the lean engine's
//! `cross_mem_link_allowed` synthesises a [`memstead_base::CreateRuleSet`]
//! on multi-folder workspaces. Only the lifecycle orchestrators —
//! `create_mem`, `delete_mem`, their param/response types, the
//! shared `NOTE_MAX_LEN` cap, and the `validate_mem_path` helper —
//! live here.
//!
//! Functions take `&mut memstead_base::Engine` directly rather than going
//! through a `FullEngine` wrapper struct: the lean engine is a single
//! polymorphic `Engine` parameterised by `Box<dyn MemBackend>` and
//! already carries every state field the orchestrators need
//! (`mem_router`, `settings`, `backend_factory`, `workspace_root`,
//! `git_branch_ops`). Full contributes lifecycle as free functions over
//! that engine; no separate engine type, no policy-provider trait.
//!
//! Return type is `Result<_, crate::FullEngineError>`. Lean-side
//! failures (`InvalidInput`, `UnknownMem`, `SchemaResolverInit`,
//! `SchemaNotFound`, `MemNameCollision`, `Mem(_)`, `Backend(_)`)
//! propagate verbatim through `FullEngineError::Lean(_)` via the
//! `#[from] memstead_base::EngineError` conversion — the `?` operator on
//! `engine.persist_state()?` and similar lean calls does the wrap
//! automatically. The four lifecycle-only variants
//! (`MemPathNotAllowed`, `MemReferencedByPolicy`, `MemSchemaNotAllowed`,
//! `ConfigAlreadyExists`) are constructed as `FullEngineError::*`
//! directly; they no longer live in `memstead_base::EngineError`.

use memstead_base::mem_management::{CreateRuleSet, DeleteRuleSet};

use crate::FullEngineError;

/// Note-length cap shared with `memstead_create` / `memstead_update` / full's
/// lifecycle orchestrators. Mirrors `memstead_git_branch::NOTE_MAX_LEN`.
pub const NOTE_MAX_LEN: usize = 280;

/// Compose an ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) for the
/// current wall clock. Used to stamp the `unregistered_at` tombstone
/// on `memstead mem unregister`. Hand-rolled to avoid a
/// chrono / time dependency — the codebase already calculates the
/// date portion in `memstead_base::entity::generator` via the same
/// epoch-day algorithm; this adds the time-of-day suffix.
fn now_iso_utc() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = dur.as_secs();
    let days = total_secs / 86_400;
    let rem = total_secs - days * 86_400;
    let hours = rem / 3_600;
    let mins = (rem % 3_600) / 60;
    let secs = rem % 60;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{mins:02}:{secs:02}Z")
}

/// Result shape of the storage-residue probe
/// `residue_probe_for_workspace` performs at Step 2b of
/// `create_mem`. `Present` carries the diagnostic payload the
/// `MEM_STORAGE_RESIDUE_DETECTED` error envelope renders, plus the
/// parsed existing config (for tombstone + reattach branches).
enum ResidueProbe {
    None,
    Present {
        branch_ref: String,
        config_blob: Option<String>,
        existing_config: Option<Box<memstead_schema::config::MemConfig>>,
    },
}

/// Probe the workspace's mem-repo for pre-existing storage at the
/// composed `branch_full_path`. Returns `None` when the workspace
/// lacks a mem-repo (folder-only) or when nothing exists at the
/// path; otherwise returns the residue payload the create-side
/// orchestrator routes against `(recovery, tombstone)` to pick a
/// path.
///
/// Implementation routes through the engine's installed backend
/// factory rather than calling `memstead-git-branch` directly — that
/// keeps `memstead-engine` decoupled from the git-branch crate (the
/// layer-above-backend posture matches the rest of `mem_management`,
/// which delegates backend instantiation through the same factory).
/// `backend.read_mem_config()` for a git-branch mount lifts
/// `__MEMSTEAD:mems/<branch_full_path>/config.json`'s bytes — present
/// iff residue exists at this exact path. Folder backends return
/// `None` here (their residue is `<location>/.memstead/config.json`
/// which Step 4 below catches separately via `ConfigAlreadyExists`).
/// Failures from the probe collapse to `None` so the create flow
/// falls through to its prior behaviour (the seed-commit step's
/// existing `HashMismatch` is the fallback safety net).
fn residue_probe_for_workspace(
    engine: &memstead_base::Engine,
    workspace_root: Option<&std::path::Path>,
    branch_full_path: &str,
    mem_name: &str,
    canonical_schema_ref: &memstead_schema::SchemaRef,
) -> ResidueProbe {
    let Some(root) = workspace_root else {
        return ResidueProbe::None;
    };
    let gitdir = root.join("mem-repo").join(".git");
    if !gitdir.is_dir() {
        return ResidueProbe::None;
    }
    let canonical_gitdir = gitdir.canonicalize().unwrap_or(gitdir);
    let probe_mount = memstead_base::workspace::Mount {
        migration_target: None,
        mem: mem_name.to_string(),
        schema: Some(canonical_schema_ref.clone()),
        storage: memstead_base::workspace::MountStorage::GitBranch {
            gitdir: canonical_gitdir,
            branch: format!("refs/heads/{branch_full_path}"),
        },
        capability: memstead_base::workspace::MountCapability::Write,
        lifecycle: memstead_base::workspace::MountLifecycle::Eager,
        cross_linkable: true,
    };
    let factory = engine.backend_factory();
    let backend = match factory(&probe_mount) {
        Ok(b) => b,
        Err(_) => return ResidueProbe::None,
    };
    let bytes = match backend.read_mem_config() {
        Ok(Some(b)) => b,
        Ok(None) | Err(_) => return ResidueProbe::None,
    };
    let existing_config = serde_json::from_slice::<memstead_schema::config::MemConfig>(&bytes)
        .ok()
        .map(Box::new);
    ResidueProbe::Present {
        branch_ref: format!("refs/heads/{branch_full_path}"),
        config_blob: Some(format!("__MEMSTEAD:mems/{branch_full_path}/config.json")),
        existing_config,
    }
}

/// Days-since-epoch → (Y, M, D). Algorithm from
/// http://howardhinnant.github.io/date_algorithms.html — same one
/// `memstead_base::entity::generator::days_to_ymd` uses; replicated here
/// to keep the function private to the orchestrator without
/// re-exporting from lean.
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ---------------------------------------------------------------------------
// `memstead_mem_delete` orchestration
// ---------------------------------------------------------------------------

/// Parameters for [`delete_mem`]. Mirrors the `memstead_mem_delete`
/// MCP tool's wire shape 1:1, plus a transport-side `operator_mode`
/// flag the wire shape does not expose.
#[derive(Debug, Clone)]
pub struct MemDeleteParams {
    /// Name of the mem to unregister. Must resolve in the current
    /// snapshot — unknown names surface as `UnknownMem`.
    pub name: String,
    /// When `true`, remove the mem's on-disk directory (folder
    /// backends only) after unregistering. Default `false` —
    /// unregister-only.
    pub delete_files: bool,
    /// Agent-authored provenance note (≤[`NOTE_MAX_LEN`] chars).
    pub note: Option<String>,
    /// Process-scoped operator-mode posture. When `true`, the
    /// orchestrator skips the `[[mem_management.delete]]` allowlist
    /// gate. Every other check (input validation, name resolution,
    /// `MEM_REFERENCED_BY_POLICY`, backend cleanup) runs
    /// identically — the policy safeguard is now gated by
    /// `delete_files: true` instead of `!operator_mode`, so the
    /// CLI's `mem delete` (operator-mode) still hits the refusal
    /// when cross-mem grants point at the target. Set only by
    /// transports that established operator intent at boot
    /// (`memstead-mcp --operator-mode`); never accepted as a wire-shape
    /// input from agents. Defaults to `false` — agent-mode.
    pub operator_mode: bool,
}

/// Response shape from [`delete_mem`].
#[derive(Debug, Clone)]
pub struct MemDeleteResponse {
    pub name: String,
    /// Always `true` on successful return — the snapshot swap
    /// happened.
    pub deleted_from_router: bool,
    /// `true` when `delete_files` was `true` AND the directory was
    /// removed cleanly; `false` otherwise (delete_files false, or
    /// removal errored, or backend has no on-disk directory).
    pub files_deleted: bool,
    /// Non-fatal findings emitted during the disk-cleanup step.
    /// Populated when `delete_files: true` was requested but
    /// [`Self::files_deleted`] ended `false` — distinguishes the
    /// mem-db-no-op case from the rmdir-failure case so an agent
    /// reading `files_deleted: false` doesn't trigger redundant
    /// cleanup attempts. Empty when nothing surprised the operation
    /// (e.g. `delete_files: false`, or `delete_files: true` and rmdir
    /// succeeded).
    pub warnings: Vec<memstead_base::ops::WarningHint>,
    /// Dangling `[cross_mem_links]` grants scrubbed from
    /// `.memstead/workspace.toml` on a destructive delete. Surfacing
    /// the scrub here gives the agent a one-round-trip view of every
    /// policy side effect. Only dangling cross-link grants are scrubbed
    /// (and reported here); the `[[mem_management.*]]` allowlist rules
    /// are preserved, so a later re-create of the same name needs no
    /// fresh `allow-create`. Empty `[]` when no cross-link grant named
    /// the deleted mem.
    pub allowlist_entries_removed: Vec<AllowlistEntryRemoved>,
}

/// One scrubbed `.memstead/workspace.toml` entry surfaced on
/// [`MemDeleteResponse::allowlist_entries_removed`]. Only dangling
/// `[cross_mem_links]` grants are scrubbed, so `table` is always
/// `"cross_mem_links"` and `from` / `to` name the directionality the
/// grant established (`from` is the table key, `to` is the array
/// element or wildcard). The `pattern` field is retained on the stable
/// response shape but is no longer populated — the
/// `[[mem_management.*]]` allowlist rules are preserved across a
/// delete and therefore never reported here.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct AllowlistEntryRemoved {
    /// Section in `.memstead/workspace.toml` the scrubbed entry came
    /// from. Always `"cross_mem_links"` — the only class scrubbed on
    /// delete.
    pub table: String,
    /// Retained on the response shape for stability but never
    /// populated since the `[[mem_management.*]]` allowlist rules
    /// are preserved across a delete. Always `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Cross-link source mem — the grant's table key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Cross-link target — the deleted mem when scrubbed from a
    /// peer's list, or `"*"` when a wildcard grant got dropped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// Unregister a writable mem at runtime. The unified-engine
/// counterpart to full's `memstead_git_branch::mem_management::delete_mem`.
///
/// Ordering guarantees mirror full's:
/// 1. Pre-mutation checks (input validation, name resolution,
///    allowlist match). Any failure here leaves the engine untouched
///    and performs zero filesystem writes.
/// 2. Router unregister (snapshot swap via
///    [`memstead_base::Engine::unregister_writable_mem`]). After this the
///    mem is no longer visible to readers. The unregister hands
///    back the backend handle so step 3 can drive backend-side
///    cleanup without re-resolving the mount.
/// 3. Optional disk delete. A failure here is non-fatal: the mem
///    is already unregistered, the leftover artifacts are a
///    follow-up concern, and the response reports `files_deleted: false`
///    with a typed `MEM_FILES_NOT_DELETED` warning naming what
///    survived.
///    - Folder mount: `remove_dir_all(location)` removes the mem
///      directory. `backend.delete_artifacts()` is a no-op (folder
///      backends keep the default impl).
///    - Mem-db-backed (git-branch) mount: `backend.delete_artifacts()`
///      drops `refs/heads/<branch_leaf>` and prunes
///      `__MEMSTEAD:mems/<branch_leaf>/config.json`. There is no
///      on-disk directory to rmdir.
///
///    `files_deleted: true` reflects "every backend-visible artifact
///    for this mem has been removed" — the wire shape is unchanged
///    but the semantic now covers both backends symmetrically.
///
/// Operator-mode bypass. When [`MemDeleteParams::operator_mode`] is
/// `true`, Step 2 (`[[mem_management.delete]]` allowlist match) is
/// skipped. All other steps run identically — including input
/// validation, name resolution, the policy safeguard, router
/// unregister, backend cleanup, and persistence. The flag is set by
/// the transport that established operator intent at boot (today,
/// `memstead-mcp --operator-mode`) and is not exposed as a wire-shape
/// input.
///
/// Policy safeguard. Step 3 (`MEM_REFERENCED_BY_POLICY`) fires when
/// `delete_files: true` AND another writable mem has a
/// `cross_mem_links` grant pointing into the target. The gating is
/// independent of `operator_mode`: storage destruction would orphan
/// the grant, so the refusal is a hard stop until the grant is
/// revoked. When `delete_files: false` (router-only unregister), the
/// storage survives and the grant remains valid against it — the
/// check is skipped. This matches the verb split exposed at the CLI
/// layer (`mem unregister` is the verb that produces `delete_files:
/// false`; `mem delete` is the verb that produces `delete_files:
/// true`).
///
/// Hierarchical candidate composition is symmetric with
/// `create_mem`: the router records the create-time `path` on
/// each writable entry, and Step 2 below reads it back via
/// [`memstead_base::MemRouterSnapshot::mem_path_for_mem`] to assemble
/// the same `<mem_path>/<name>` (or bare `<name>`) string the
/// create-side composer matched against.
pub fn delete_mem(
    engine: &mut memstead_base::Engine,
    params: MemDeleteParams,
) -> Result<MemDeleteResponse, FullEngineError> {
    // ---- Step 0: input validation ----
    if let Some(note) = params.note.as_deref()
        && note.chars().count() > NOTE_MAX_LEN
    {
        return Err(memstead_base::EngineError::InvalidInput(format!(
            "note exceeds {NOTE_MAX_LEN} characters"
        ))
        .into());
    }

    // ---- Step 1: resolve name ----
    if !engine.mem_router().is_writable(&params.name) {
        return Err(memstead_base::EngineError::UnknownMem(params.name.clone()).into());
    }

    // ---- Step 2: allowlist match ----
    // Snapshot the on-disk dir (folder mounts only) and the
    // create-time hierarchical `mem_path`. The lifecycle candidate
    // composes as `<mem_path>/<name>` when a path was recorded at
    // registration, falling back to the bare `<name>` for flat-layout
    // mems — the same shape `create_mem` matched against the rule
    // list. Symmetric on the engine side; closes the asymmetry that
    // previously left hierarchical runtime mems un-deletable.
    //
    // Operator-mode skips the allowlist match entirely. The mem_dir
    // is still resolved because Step 5 needs it for the rmdir step.
    let mem_dir: Option<std::path::PathBuf> = engine
        .mem_router()
        .dir_for_mem(&params.name)
        .map(|p| p.to_path_buf());

    if !params.operator_mode {
        // Hierarchical paths are first-class mem identifiers.
        // `params.name` is already the full path (e.g.
        // `team/sub-mem`) — no separate `mem_path` composition
        // step needed. The delete-side lifecycle candidate IS the
        // mem name.
        let attempted = mem_dir
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from(format!("(mem: {})", params.name)));
        let candidate: String = params.name.clone();

        let delete_rule_set = DeleteRuleSet::new(engine.settings().mem_delete_rules.clone())
            .map_err(|e| {
                memstead_base::EngineError::InvalidInput(format!("mem_delete_rules: {e}"))
            })?;
        let patterns_for_errors: Vec<String> = delete_rule_set.patterns();

        if delete_rule_set.is_empty() {
            return Err(FullEngineError::MemPathNotAllowed {
                attempted,
                candidate,
                patterns: patterns_for_errors,
                reason: "no_allowlist_configured",
                policy_table: "mem_management.delete",
            });
        }
        if delete_rule_set
            .first_match(std::path::Path::new(&candidate))
            .is_none()
        {
            return Err(FullEngineError::MemPathNotAllowed {
                attempted,
                candidate,
                patterns: patterns_for_errors,
                reason: "no_match",
                policy_table: "mem_management.delete",
            });
        }
    }

    // ---- Step 3: MEM_REFERENCED_BY_POLICY check ----
    // Walk the workspace's `cross_mem_links` setting and refuse to
    // delete a mem that any other visible writable mem is
    // permitted to link into. Reads the workspace-level policy
    // directly rather than per-mem `effective_cross_links` (the
    // per-mem projection that composes workspace policy with
    // create-rule defaults) — for workspaces that only configure
    // links via the workspace-level `[cross_mem_links]` section
    // (the common case), the two are equivalent.
    //
    // The check gates on `delete_files: true` rather than
    // `!operator_mode`. The
    // safeguard protects against orphaning a grant by destroying the
    // storage it relies on; when `delete_files: false` (router-only
    // unregister), the storage survives so grants remain valid and
    // re-activate on re-init. The new CLI verb `memstead mem
    // unregister` maps to this branch unconditionally; the CLI verb
    // `memstead mem delete` maps to the storage-destruction branch
    // where the check fires regardless of `operator_mode` (the
    // operator is presumed to have surveyed the link graph, but a
    // hard stop forces an explicit revoke-first flow).
    if params.delete_files {
        use memstead_schema::workspace_config::CrossLinkValue;
        let mut referring_mems: Vec<String> = engine
            .settings()
            .cross_mem_links
            .iter()
            .filter_map(|(referring, policy)| {
                if referring == &params.name {
                    return None;
                }
                match policy {
                    CrossLinkValue::List(targets) => {
                        if targets.iter().any(|t| t == &params.name) {
                            Some(referring.clone())
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            })
            .collect();
        referring_mems.sort();
        referring_mems.dedup();
        if !referring_mems.is_empty() {
            return Err(FullEngineError::MemReferencedByPolicy {
                name: params.name,
                referring_mems,
            });
        }
    }

    // ---- Step 3a: MEM_HAS_INCOMING_REFS check ----
    // The policy check above closes the workspace-policy axis ("is this
    // mem still grant-pointed-at?") but not the edge-graph axis
    // ("does any actual entity still point at this mem's entities?").
    // Revoking a grant is independent of removing the edges. Without
    // this step, a mem whose grant was revoked but whose surviving
    // Write-Mem peers still carry `DEPENDS_ON → target_mem--*`
    // edges deletes cleanly, leaving dangling cross-mem edges that
    // resolve to nothing.
    //
    // The scan walks every entity in the doomed mem, collects each
    // entity's incoming edges, partitions out same-mem and ReadOnly-
    // mount referrers, and groups the remaining Write-Mem referrers
    // by source-entity id (one [`memstead_base::ReferrerInfo`] per source,
    // `rel_types` aggregating every offending edge type). Same shape
    // as entity-level `HasIncomingRefs` — see [`memstead_base::EngineError::MemHasIncomingRefs`]
    // for the refusal-with-recovery contract. Fires regardless of
    // `delete_files` because a router-only unregister with stale edges
    // is just as broken as a storage-destruction with stale edges —
    // either way, surviving entities point at a mem the engine no
    // longer routes to.
    {
        use std::collections::BTreeSet;

        // Source-id is grouped via a `Vec` of `(EntityId, BTreeSet<rel_type>)`
        // pairs keyed by the id's string form — EntityId itself isn't
        // `Ord` (no full lexical ordering defined), but its string
        // form sorts deterministically and is what the wire envelope
        // serialises anyway.
        let store = engine.store();
        let mut by_source: std::collections::BTreeMap<
            String,
            (memstead_base::EntityId, BTreeSet<String>),
        > = std::collections::BTreeMap::new();
        let doomed_mem = params.name.as_str();
        for entity in store.all_entities() {
            if entity.mem != doomed_mem {
                continue;
            }
            for in_edge in store.incoming(&entity.id) {
                if in_edge.from.mem() == doomed_mem {
                    continue;
                }
                // ReadOnly-mount referrers are partitioned out — they
                // route through the residual-stub demotion path on
                // the destructive mutation, same as entity-level
                // HasIncomingRefs. Use the router's capability lookup
                // to decide; mounts the router doesn't know about
                // (shouldn't happen post-construction) are treated as
                // Write to be conservative.
                let is_writable = engine.mem_router().is_writable(in_edge.from.mem());
                if !is_writable {
                    continue;
                }
                by_source
                    .entry(in_edge.from.to_string())
                    .or_insert_with(|| (in_edge.from.clone(), BTreeSet::new()))
                    .1
                    .insert(in_edge.rel_type.clone());
            }
        }

        if !by_source.is_empty() {
            let referrers: Vec<memstead_base::ReferrerInfo> = by_source
                .into_values()
                .map(|(from, rel_types)| memstead_base::ReferrerInfo {
                    from_id: from.to_string(),
                    rel_types: rel_types.into_iter().collect(),
                    mem: from.mem().to_string(),
                })
                .collect();
            return Err(memstead_base::EngineError::MemHasIncomingRefs {
                mem: params.name,
                referrers,
            }
            .into());
        }
    }

    // ---- Step 4: router unregister ----
    // Returns the backend handle so step 5 can drive backend-side
    // cleanup without re-resolving the mount through the router
    // (which has already lost the entry by the time we get here).
    let removed_backend = engine.unregister_writable_mem(&params.name)?;
    let backend =
        removed_backend.expect("mem_router().is_writable check above guarantees a present mount");

    // ---- Step 4b: tombstone write (unregister-only path) ----
    // When the operator asked for
    // router-only removal (`delete_files: false`), stamp the surviving
    // config blob with an `unregistered_at` ISO-8601 marker so a
    // subsequent `memstead mem init <same-name>` can recognize the
    // residue as deliberate operator state (zero-friction reattach
    // path) versus crash residue (refuse with
    // `MEM_STORAGE_RESIDUE_DETECTED`).
    //
    // Failures here are non-fatal — the unregister has already
    // committed, and a missing tombstone only downgrades the
    // re-init flow (operator must pass `--reattach` explicitly).
    // The warning surfaces the missed write so the operator can
    // intervene if needed.
    if !params.delete_files {
        match backend.read_mem_config() {
            Ok(Some(bytes)) => {
                match serde_json::from_slice::<memstead_schema::config::MemConfig>(&bytes) {
                    Ok(mut cfg) => {
                        cfg.unregistered_at = Some(now_iso_utc());
                        match serde_json::to_vec_pretty(&cfg) {
                            Ok(mut new_bytes) => {
                                new_bytes.push(b'\n');
                                if let Err(e) = backend.write_mem_config(&new_bytes) {
                                    tracing::warn!(
                                        mem = %params.name,
                                        error = %e,
                                        "delete_mem: unregister succeeded but tombstone \
                                         write failed — re-init will require an explicit \
                                         --reattach flag"
                                    );
                                }
                            }
                            Err(e) => tracing::warn!(
                                mem = %params.name,
                                error = %e,
                                "delete_mem: tombstone serialize failed",
                            ),
                        }
                    }
                    Err(e) => tracing::warn!(
                        mem = %params.name,
                        error = %e,
                        "delete_mem: tombstone-write skipped — config blob did not \
                         parse as MemConfig",
                    ),
                }
            }
            Ok(None) => {
                // No on-disk config blob (folder backend with no
                // `.memstead/config.json`, or git-branch mount whose
                // `__MEMSTEAD:mems/.../config.json` was never written).
                // Nothing to stamp.
            }
            Err(e) => tracing::warn!(
                mem = %params.name,
                error = %e,
                "delete_mem: tombstone-read skipped — backend read_mem_config errored",
            ),
        }
    }

    // ---- Step 5: optional disk delete ----
    // `delete_files: true` runs BOTH halves of the symmetric cleanup:
    //   1. Backend-side `delete_artifacts()`. Folder + archive
    //      backends keep the default no-op. The git-branch backend
    //      drops `refs/heads/<branch_leaf>` and prunes
    //      `__MEMSTEAD:mems/<branch_leaf>/config.json` in a single
    //      ref-edit transaction.
    //   2. Folder-direct `remove_dir_all(location)` when the mount
    //      registered an on-disk directory (folder backends only).
    //      Git-branch backends register `dir: None` so this branch
    //      is skipped — the backend step above handled their state.
    // Any sub-step failing leaves the operation in a documented
    // partial state: the mem is already unregistered, `files_deleted`
    // ends `false`, and per-failure `MEM_FILES_NOT_DELETED` warnings
    // name the surviving artifact(s). `delete_files: false` returns
    // `files_deleted: false` silently (the archive-workflow contract).
    let mut warnings: Vec<memstead_base::ops::WarningHint> = Vec::new();
    let files_deleted = if params.delete_files {
        let backend_ok = match backend.delete_artifacts() {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(
                    mem = %params.name,
                    error = %e,
                    "delete_mem: unregister succeeded but backend artifact \
                     cleanup failed — leaving leftover refs / tree entries \
                     for explicit cleanup"
                );
                warnings.push(memstead_base::ops::WarningHint::MemFilesNotDeleted {
                    mem: params.name.clone(),
                    reason: "backend_prune_failed".into(),
                    path: None,
                    error: Some(e.to_string()),
                });
                false
            }
        };
        let dir_ok = match mem_dir.as_ref() {
            Some(dir) => match std::fs::remove_dir_all(dir) {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(
                        mem = %params.name,
                        path = %dir.display(),
                        error = %e,
                        "delete_mem: unregister succeeded but rmdir failed — \
                         leaving leftover files for explicit cleanup"
                    );
                    warnings.push(memstead_base::ops::WarningHint::MemFilesNotDeleted {
                        mem: params.name.clone(),
                        reason: "rmdir_failed".into(),
                        path: Some(dir.display().to_string()),
                        error: Some(e.to_string()),
                    });
                    false
                }
            },
            // No on-disk directory to rmdir — mem-db-backed mount.
            // The backend step above carried the cleanup; no warning
            // here since the absence of a directory is the documented
            // shape, not a partial-state signal.
            None => true,
        };
        backend_ok && dir_ok
    } else {
        false
    };

    // Symmetric persistence with `create_mem`: write the post-
    // unregister mount manifest so a sibling process boots without
    // the deleted mem. If the workspace_root is unset (tests /
    // ad-hoc consumers) the call is a no-op.
    engine.persist_state()?;

    // ---- Step 6: policy scrub on destructive delete ----
    // When both refusal gates admitted and the destructive delete
    // committed, scrub
    // `.memstead/workspace.toml` of the now-dangling `[cross_mem_links]`
    // grants naming the deleted mem — its own key plus every peer's
    // allowlist value. The `[[mem_management.create|delete]]`
    // allowlist rules are deliberately preserved (forward-looking
    // permissions for the name, not references to the gone instance),
    // so a later `mem init <same name>` needs no fresh allow-create.
    // Refresh the engine's in-memory settings from the freshly-edited
    // workspace.toml so a follow-up `memstead_mem_create` against the
    // same name doesn't trip a stale grant, and `workspace show`
    // agrees with the on-disk file. Skipped for router-only unregister
    // (`delete_files: false`): the storage and grants survive
    // together, set to re-activate on a future reattach.
    let mut allowlist_entries_removed: Vec<AllowlistEntryRemoved> = Vec::new();
    if params.delete_files
        && let Some(root) = engine.workspace_root().map(|p| p.to_path_buf())
    {
        // Scrub failures are non-fatal but surfaced as warnings —
        // the delete itself committed, and dangling policy entries
        // would only cost reload-time `UNKNOWN_MEM` checks. Wrap
        // the typed enum in a warning code the agent can branch on.
        match crate::workspace_config_edit::scrub_policy_for_deleted_mem(&root, &params.name) {
            Err(e) => {
                tracing::warn!(
                    mem = %params.name,
                    error = %e,
                    "delete_mem: destructive delete committed but policy \
                     scrub failed — `.memstead/workspace.toml` may still \
                     reference the deleted mem"
                );
            }
            Ok(scrubbed) => {
                // Lift scrubbed entries into the response envelope
                // so the agent doesn't have to re-read
                // `workspace show` to learn the side effects of
                // the delete.
                allowlist_entries_removed = scrubbed
                    .into_iter()
                    .map(|e| match e {
                        crate::workspace_config_edit::ScrubbedEntry::CrossLink { from, to } => {
                            AllowlistEntryRemoved {
                                table: "cross_mem_links".to_string(),
                                pattern: None,
                                from: Some(from),
                                to: Some(to),
                            }
                        }
                    })
                    .collect();
                // Refresh the in-memory settings so the scrub takes
                // effect without a full reload. Best-effort: missing or
                // unparseable file leaves the existing in-memory
                // settings untouched (the scrub already succeeded; the
                // pre-scrub settings were strictly more permissive).
                let store = memstead_base::workspace_store::FileWorkspaceStore::new();
                if let Ok(ws) = <memstead_base::workspace_store::FileWorkspaceStore as memstead_base::workspace_store::WorkspaceStoreAdapter>::load(
                        &store,
                        &root,
                    ) {
                        engine.set_settings(ws.settings);
                    }
            }
        }
    }

    // `require_notes` provenance nudge — inherited from the engine's
    // single enforcement point (see `create_mem`).
    if let Some(w) = engine.note_missing_warning("delete_mem", params.note.as_deref()) {
        warnings.push(w);
    }

    Ok(MemDeleteResponse {
        name: params.name,
        deleted_from_router: true,
        files_deleted,
        warnings,
        allowlist_entries_removed,
    })
}

// ---------------------------------------------------------------------------
// `memstead_mem_create` orchestration
// ---------------------------------------------------------------------------

/// Parameters for [`create_mem`]. Mirrors the `memstead_mem_create`
/// MCP tool's wire shape 1:1.
///
/// Hierarchical paths are first-class mem identifiers — there is no
/// separate `path` field; `name` carries the full path
/// (e.g. `"team/sub-mem"`) directly, validated via
/// [`memstead_base::entity::id::validate_mem_name_grammar`]. The
/// branch ref composes as `refs/heads/<name>` and the `__MEMSTEAD`
/// config blob as `__MEMSTEAD:mems/<name>/config.json` with no extra
/// composition step.
#[derive(Debug, Clone)]
pub struct MemCreateParams {
    /// Name of the new mem — the full hierarchical identifier
    /// (e.g. `"sub-mem"` for flat layouts or `"team/sub-mem"`
    /// for hierarchical layouts). Must be unique across every
    /// visible mem in the current snapshot, match the basename of
    /// `location` (folder backend identity invariant on the
    /// trailing path segment), and satisfy the mem-name grammar
    /// (`[a-z0-9-]+(/[a-z0-9-]+)*`).
    pub name: String,
    /// Target location. Absolute or workspace-relative.
    /// Canonicalized inside the orchestrator before the allowlist
    /// check.
    pub location: std::path::PathBuf,
    /// Schema pin for the new mem (`name@x.y.z`).
    pub schema_ref: memstead_schema::SchemaRef,
    /// Optional vcs config override (signing keys, identity hints).
    /// Persisted into the per-mem config blob alongside `schema`
    /// and `name`. Most callers pass `None` — defaults come from
    /// the workspace's `.memstead/workspace.toml`. Mirrors full's
    /// `memstead_git_branch::MemCreateParams.vcs`.
    pub vcs: Option<memstead_schema::VcsConfig>,
    /// Agent-authored provenance note (≤[`NOTE_MAX_LEN`] chars).
    pub note: Option<String>,
    /// Process-scoped operator-mode posture. When `true`, the
    /// orchestrator skips the `[[mem_management.create]]` allowlist
    /// gate and the matched-rule schema gate it derives — every other
    /// check (input validation, schema canonicalisation, basename
    /// invariant, name collision, backend instantiation) runs
    /// identically. Set only by transports that established operator
    /// intent at boot (`memstead-mcp --operator-mode`); never accepted as
    /// a wire-shape input from agents. Defaults to `false` —
    /// agent-mode.
    pub operator_mode: bool,
    /// Explicit recovery action for
    /// the case where the storage already carries residue for the
    /// composed branch path. `None` is the default — the engine
    /// then routes by tombstone presence: residue with an
    /// `unregistered_at` tombstone (deliberate operator state from
    /// `memstead mem unregister`) defaults to [`RecoveryAction::Reattach`],
    /// residue without a tombstone refuses with
    /// `MEM_STORAGE_RESIDUE_DETECTED`. Setting an explicit value
    /// overrides the tombstone-driven default. A bare `mem init`
    /// against a name with no residue at all ignores this field
    /// (the happy path is unchanged).
    pub recovery: Option<crate::RecoveryAction>,
    /// Optional opaque per-instance writing guidance, persisted
    /// verbatim into the new mem's config (`writeGuidance`) in the
    /// seed commit. The engine never inspects the map's contents
    /// (schema-strictness D8 — `writeGuidance` is client-owned
    /// vocabulary); a client that read a schema package's
    /// `mem-template.json` fills the instance keys and passes them
    /// here. Empty map (the default) seeds no guidance, identical to
    /// pre-parameter behaviour.
    pub write_guidance: std::collections::HashMap<String, serde_json::Value>,
}

/// Response shape from [`create_mem`].
#[derive(Debug, Clone)]
pub struct MemCreateResponse {
    pub name: String,
    pub location: std::path::PathBuf,
    pub schema_ref: memstead_schema::SchemaRef,
    /// Seed-commit cursor. Folder backends produce a synthetic id
    /// (UNIX-nanos + counter, hex per the trait's contract);
    /// git-branch backends produce a real 40-char hex sha. Either
    /// way, the cursor is non-empty — agents poll
    /// `memstead_changes_since` against it without branching on
    /// backend type. Empty string (`""`) signals the reattach branch
    /// was taken — pair with the `MEM_REATTACHED_AFTER_UNREGISTER`
    /// warning surfaced via [`Self::warnings`] for full context.
    pub seed_commit_sha: String,
    /// Non-fatal findings emitted during the create / reattach
    /// pipeline. Today populated only by the reattach branch with a
    /// `MEM_REATTACHED_AFTER_UNREGISTER` warning carrying
    /// `{mem, unregistered_at}`. Fresh-create
    /// (residue absent or force-overwrite branch taken) leaves this
    /// empty.
    pub warnings: Vec<memstead_base::ops::WarningHint>,
}

/// Classify a structurally-invalid mem name into the typed
/// `FullEngineError::InvalidMemName.reason` discriminator. Returns
/// `None` when the name passes the structural check and the caller
/// should proceed to the regex-level grammar check + allowlist gate.
///
/// Discriminator vocabulary:
/// - `empty` — `params.name == ""`.
/// - `whitespace` — input contains any ASCII whitespace, or is non-
///   empty but trims to empty.
/// - `reserved_prefix` — any path segment starts with `__` (the
///   reserved prefix the engine uses for `__MEMSTEAD` registry refs and
///   similar). Caught early so the operator sees the intent rather
///   than a regex no-match.
/// - `invalid_char` — fallback for anything else the grammar rejects
///   (non-printable, non-ASCII letters, reserved characters).
fn classify_invalid_mem_name(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("empty");
    }
    if name.chars().any(char::is_whitespace) {
        return Some("whitespace");
    }
    if name.split('/').any(|seg| seg.starts_with("__")) {
        return Some("reserved_prefix");
    }
    None
}

/// Create a new writable mem at runtime. Unified counterpart to
/// full's `memstead_git_branch::mem_management::create_mem`. Routes
/// through the engine's installed [`memstead_base::BackendFactory`] so the
/// same call site materialises folder, archive, or git-branch
/// backends transparently — production full consumers install
/// `memstead_git_branch::storage::instantiate_full_backend` at boot via
/// `engine_from_workspace_root`.
///
/// Pipeline:
/// 0. Input validation (note length, optional `path` segments).
///    - 0b. Schema canonicalization against the built-in catalogue.
///      Workspace-authored schemas are resolved by the workspace's
///      schemas_dir, not yet here.
/// 1. Canonicalize location against `engine.workspace_root()`
///    (relative paths) or take absolute paths as-is.
///    - 1a. Allowlist match against the composed candidate
///      (`<path>/<name>` when `path` is `Some`, else `<name>`).
///    - 1b. Schema gate against matched rule's `schemas` list. `["*"]`
///      wildcard admits any schema.
///    - 1c. Basename invariant: `params.name` MUST equal the canonical
///      location's basename.
/// 2. Name collision probe against the current mem_router
///    snapshot. The rich tree-walk collision detector (with
///    `colliding_paths` envelope payload) is full-only; unified
///    surfaces collisions through the snapshot probe with the
///    same `EngineError::MemNameCollision` discriminant.
/// 3. Build [`memstead_schema::config::MemConfig`] bytes.
///    - 3b. Pick the storage variant by workspace shape: git-branch
///      when `<workspace_root>/mem-repo/.git/` exists; folder
///      otherwise. The branch leaf composes as `<path>/<name>`.
/// 4. Write `<location>/.memstead/config.json` for folder mounts
///    only. Git-branch mounts skip the on-disk write — the
///    per-mem config travels in the workspace's `__MEMSTEAD` registry
///    ref.
/// 5. Materialise the backend via the engine's
///    [`memstead_base::BackendFactory`], commit the seed (real sha for
///    git-branch, synthetic id for folder), and register via
///    [`memstead_base::Engine::register_writable_mem`] with
///    [`memstead_base::MemOrigin::RuntimeCreated`].
pub fn create_mem(
    engine: &mut memstead_base::Engine,
    mut params: MemCreateParams,
) -> Result<MemCreateResponse, FullEngineError> {
    use std::path::Path;

    // ---- Step 0: input validation ----
    if let Some(note) = params.note.as_deref()
        && note.chars().count() > NOTE_MAX_LEN
    {
        return Err(memstead_base::EngineError::InvalidInput(format!(
            "note exceeds {NOTE_MAX_LEN} characters"
        ))
        .into());
    }
    // Hierarchical paths are first-class. `params.name` carries the
    // full path (e.g. `"team/sub-mem"`); the grammar validator
    // accepts both flat and hierarchical forms and refuses
    // malformations (leading / trailing / double slashes, segments
    // outside `[a-z0-9-]+`).
    //
    // Structural-failure modes get typed reasons before the allowlist
    // check fires, so the four distinct shapes (empty / whitespace /
    // invalid-char / reserved-prefix) stay distinguishable from a
    // legitimate authorisation refusal rather than collapsing into the
    // post-allowlist `MEM_PATH_NOT_ALLOWED (no_match)` envelope.
    if let Some(reason) = classify_invalid_mem_name(&params.name) {
        return Err(FullEngineError::InvalidMemName {
            name: params.name.clone(),
            reason,
        });
    }
    // Grammar check (regex-level shape) for anything the structural
    // classifier did not catch. The grammar refusals shouldn't fire
    // here in practice — `classify_invalid_mem_name` already covers
    // every concrete malformation. But keep the call as a defense in
    // depth in case the grammar tightens later; route through the
    // typed `invalid_char` reason so the wire shape stays consistent.
    if memstead_base::entity::id::validate_mem_name_grammar(&params.name).is_err() {
        return Err(FullEngineError::InvalidMemName {
            name: params.name.clone(),
            reason: "invalid_char",
        });
    }

    // ---- Step 0b: schema canonicalization ----
    // Resolve the agent-supplied schema pin against the engine's full
    // loaded catalogue: workspace-authored schemas from the backend's
    // local storage (folder `.memstead/schemas/` or the git-branch
    // `__MEMSTEAD:schemas/` ref) layered over the built-ins. A mem can
    // therefore pin a schema installed onto the backend via
    // `memstead schema install`, not just a built-in.
    let mut builtin_schemas: Vec<std::sync::Arc<memstead_schema::Schema>> =
        engine.workspace_schemas().to_vec();
    builtin_schemas.extend_from_slice(engine.builtin_schemas());
    let resolved_schema = memstead_base::engine::SchemaResolver::new(&builtin_schemas)
        .resolve(&params.schema_ref)
        .map_err(|sources| memstead_base::EngineError::SchemaNotFound {
            mem: params.name.clone(),
            pin: params.schema_ref.to_string(),
            sources,
        })?;
    let canonical_schema_ref = memstead_schema::SchemaRef::new(
        resolved_schema.manifest.name.clone(),
        resolved_schema.version.clone(),
    );
    params.schema_ref = canonical_schema_ref.clone();

    // ---- Step 1: canonicalize location ----
    let workspace_root = engine.workspace_root().map(|p| p.to_path_buf());
    let absolute = if params.location.is_absolute() {
        params.location.clone()
    } else if let Some(root) = workspace_root.as_ref() {
        root.join(&params.location)
    } else {
        // No workspace_root set (tests, ad-hoc consumers). Treat
        // relative as relative-to-CWD by canonicalising directly;
        // the basename invariant + allowlist still apply.
        params.location.clone()
    };
    let canonical = canonicalize_maybe_missing(&absolute);

    // ---- Step 1a: allowlist match ----
    // Compose the allowlist candidate from the optional `<path>`
    // plus `<name>`. Flat layout (path = None) candidate is just
    // `<name>`; hierarchical layout candidate is `<path>/<name>`
    // (used by the rule lookup and surfaced in the
    // `MEM_PATH_NOT_ALLOWED` envelope's `details.candidate` field).
    //
    // Operator-mode bypasses Step 1a and Step 1b entirely. The
    // outside-workspace check below also folds in — operators
    // typically rebuild from scratch and may place a mem outside
    // any allowlist'd region. Every safety-shaped check (schema
    // canonicalisation, basename invariant, name collision) stays
    // unconditional.
    // Hierarchical paths are first-class. The allowlist candidate IS
    // the mem name (no `<path>/<name>` composition step —
    // `params.name` already carries the full path).
    let candidate: String = params.name.clone();

    if !params.operator_mode {
        let create_rule_set = CreateRuleSet::new(engine.settings().mem_create_rules.clone())
            .map_err(|e| {
                memstead_base::EngineError::InvalidInput(format!("mem_create_rules: {e}"))
            })?;
        let patterns_for_errors: Vec<String> = create_rule_set.patterns();

        if create_rule_set.is_empty() {
            return Err(FullEngineError::MemPathNotAllowed {
                attempted: canonical.clone(),
                candidate,
                patterns: patterns_for_errors,
                reason: "no_allowlist_configured",
                policy_table: "mem_management.create",
            });
        }
        let matched_rule = match create_rule_set.first_match(Path::new(&candidate)) {
            Some(r) => r.clone(),
            None => {
                return Err(FullEngineError::MemPathNotAllowed {
                    attempted: canonical.clone(),
                    candidate,
                    patterns: patterns_for_errors,
                    reason: "no_match",
                    policy_table: "mem_management.create",
                });
            }
        };

        // Outside-workspace check (skipped when no workspace_root is
        // set — tests / ad-hoc).
        if let Some(root) = workspace_root.as_ref()
            && canonical.strip_prefix(root).is_err()
        {
            return Err(FullEngineError::MemPathNotAllowed {
                attempted: canonical.clone(),
                candidate,
                patterns: patterns_for_errors,
                reason: "outside_workspace",
                policy_table: "mem_management.create",
            });
        }

        // ---- Step 1b: schema gate ----
        let schema_wildcard = matched_rule
            .schemas
            .iter()
            .any(|s| s == memstead_base::SCHEMA_WILDCARD);
        if !schema_wildcard {
            let requested_canonical = canonical_schema_ref.to_string();
            let mut allowed_canonical: Vec<String> = Vec::with_capacity(matched_rule.schemas.len());
            let mut allowed = false;
            for raw in &matched_rule.schemas {
                let parsed: memstead_schema::SchemaRef = match raw.parse() {
                    Ok(r) => r,
                    Err(_) => {
                        return Err(memstead_base::EngineError::InvalidInput(format!(
                            "[mem_management] rule {:?}: schema entry {:?} is not a valid `name@version` pin",
                            matched_rule.pattern, raw,
                        ))
                        .into());
                    }
                };
                let resolved = memstead_base::engine::SchemaResolver::new(&builtin_schemas)
                    .resolve(&parsed)
                    .map_err(|sources| memstead_base::EngineError::SchemaNotFound {
                        mem: params.name.clone(),
                        pin: parsed.to_string(),
                        sources,
                    })?;
                let canon_str = memstead_schema::SchemaRef::new(
                    resolved.manifest.name.clone(),
                    resolved.version.clone(),
                )
                .to_string();
                if canon_str == requested_canonical {
                    allowed = true;
                }
                allowed_canonical.push(canon_str);
            }
            if !allowed {
                return Err(FullEngineError::MemSchemaNotAllowed {
                    candidate,
                    matched_pattern: matched_rule.pattern.clone(),
                    requested_schema: requested_canonical,
                    allowed_schemas: allowed_canonical,
                });
            }
        }
    }

    // ---- Step 1c: basename invariant ----
    // Skipped on the git-branch path (mem-repo workspaces) because
    // the equivalent invariant is implicit there: `params.name` IS
    // the branch identifier, and `params.location` is ignored at
    // runtime (the mem has no on-disk identity beyond the gitdir).
    //
    // Mem names accept hierarchical paths (`team/sub-mem`). The
    // on-disk basename matches the LAST segment of the path —
    // folder-backed
    // hierarchical mems register under `<location>/sub-mem`
    // even when their identity is `team/sub-mem`.
    let workspace_has_mem_repo = workspace_root
        .as_ref()
        .map(|root| root.join("mem-repo").join(".git").is_dir())
        .unwrap_or(false);
    if !workspace_has_mem_repo {
        let target_basename = canonical
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let name_leaf = params
            .name
            .rsplit('/')
            .next()
            .unwrap_or(params.name.as_str());
        if target_basename != name_leaf {
            return Err(memstead_base::EngineError::InvalidInput(format!(
                "mem name '{}' (leaf '{}') does not match the basename '{}' of the canonical location '{}' \
                 — rename either side so the registered identity's leaf matches the on-disk basename",
                params.name,
                name_leaf,
                target_basename,
                canonical.display()
            ))
            .into());
        }
    }

    // ---- Step 2: name collision probe (snapshot only) ----
    if let Some(existing) = engine.mem_router().origin_for_mem(&params.name) {
        return Err(memstead_base::EngineError::MemNameCollision {
            name: params.name,
            source_origin: existing.render_source(),
        }
        .into());
    }
    if engine
        .mem_router()
        .archive_path_for_mem(&params.name)
        .is_some()
    {
        return Err(memstead_base::EngineError::MemNameCollision {
            name: params.name,
            source_origin: "attached read mem".to_string(),
        }
        .into());
    }

    // ---- Step 2b: storage residue probe (mem-repo only) ----
    // A name absent from the in-memory router can still have storage
    // residue — a per-mem content branch +
    // `__MEMSTEAD:mems/<branch_leaf>/config.json` blob surviving a
    // `memstead mem unregister` (deliberate operator state), a crash
    // mid-create, or a partially-failed delete. Without this probe the
    // seed-commit step would silently re-attach (resurrecting deleted
    // entities) or fail with a low-level `HashMismatch` carrying no
    // useful recovery context. The probe here is path-aware: the
    // composed `branch_leaf`
    // (`<mem_path>/<name>` for hierarchical, bare `<name>` for
    // flat) is the exact branch ref to inspect — `find_branches_by_leaf`
    // is too permissive (would match `other-team/<name>` for
    // `team/<name>`).
    //
    // Folder-backed workspaces don't have a branch-residue concept;
    // their analogous probe is "does `<location>/.memstead/config.json`
    // already exist?" — which Step 4 below already enforces via
    // `ConfigAlreadyExists`. The residue refusal is git-branch-only.
    // Hierarchical paths are first-class: the composed branch path IS
    // the mem name
    // (`params.name` carries the full `team/sub-mem` form
    // directly). Bound to a local for readability and to match the
    // reattach + force-overwrite arm shapes that still need a
    // `&str` reference.
    let composed_branch_leaf = params.name.clone();
    let residue_probe = residue_probe_for_workspace(
        engine,
        workspace_root.as_deref(),
        &composed_branch_leaf,
        &params.name,
        &canonical_schema_ref,
    );
    // The match discriminates the residue routes: `None` / fresh-create
    // and `ForceOverwrite` fall through to Step 3 below; `Reattach`
    // early-returns with the warning surfaced via the response's
    // `warnings` field. The match value itself is unused once those
    // branches have taken effect — the warning emission lives on the
    // response, not the discarded binding.
    let _: Option<memstead_base::ops::WarningHint> = match residue_probe {
        ResidueProbe::None => None,
        ResidueProbe::Present {
            branch_ref,
            config_blob,
            existing_config,
        } => {
            let tombstone = existing_config
                .as_ref()
                .and_then(|c| c.unregistered_at.clone());
            let effective_action = params
                .recovery
                .or_else(|| tombstone.as_ref().map(|_| crate::RecoveryAction::Reattach));
            match effective_action {
                None => {
                    return Err(FullEngineError::MemStorageResidueDetected {
                        branch_ref,
                        config_blob,
                        entity_count: 0,
                    });
                }
                Some(crate::RecoveryAction::HardCleanupFirst) => {
                    return Err(FullEngineError::MemStorageResidueDetected {
                        branch_ref,
                        config_blob,
                        entity_count: 0,
                    });
                }
                Some(crate::RecoveryAction::ForceOverwrite) => {
                    // Force-overwrite — prune the residual branch and
                    // `__MEMSTEAD` config blob in one ref-edit
                    // transaction, then fall through to Steps 3-5
                    // (normal create path). The match arm yields
                    // `None` so no warning rides on the response;
                    // the prior entities are gone by design.
                    let workspace_root_ref = workspace_root.as_ref().ok_or_else(|| {
                        memstead_base::EngineError::InvalidInput(
                            "force_overwrite requires a workspace_root \
                                 to locate mem-repo/.git/"
                                .to_string(),
                        )
                    })?;
                    let gitdir = workspace_root_ref.join("mem-repo").join(".git");
                    let canonical_gitdir = gitdir.canonicalize().unwrap_or(gitdir);
                    let ops = engine.git_branch_ops().ok_or_else(|| {
                        memstead_base::EngineError::InvalidInput(
                            "force_overwrite requires the git-branch ops \
                             bundle (full boot only) — folder workspaces \
                             have no branch residue to prune"
                                .to_string(),
                        )
                    })?;
                    (ops.prune_residue)(&canonical_gitdir, &composed_branch_leaf).map_err(|e| {
                        memstead_base::EngineError::Mem(format!("force_overwrite prune: {e}"))
                    })?;
                    // Fall through to Step 3 — the residue is gone,
                    // create proceeds normally and the fresh seed
                    // commit is the new branch tip.
                    None
                }
                Some(crate::RecoveryAction::Reattach) => {
                    // Reattach path — register the existing branch
                    // as a fresh writable mount, skip the seed
                    // commit (the branch already carries history),
                    // clear the tombstone if present, surface the
                    // audit warning. Falls out below via early
                    // return so steps 3-5 stay aligned with the
                    // fresh-create path.
                    let workspace_root_ref = workspace_root.as_ref().ok_or_else(|| {
                        memstead_base::EngineError::InvalidInput(
                            "reattach requires a workspace_root \
                                 to locate mem-repo/.git/"
                                .to_string(),
                        )
                    })?;
                    let gitdir = workspace_root_ref.join("mem-repo").join(".git");
                    let canonical_gitdir = gitdir.canonicalize().unwrap_or(gitdir);
                    let mount = memstead_base::workspace::Mount {
                        migration_target: None,
                        mem: params.name.clone(),
                        schema: Some(canonical_schema_ref.clone()),
                        storage: memstead_base::workspace::MountStorage::GitBranch {
                            gitdir: canonical_gitdir,
                            branch: format!("refs/heads/{composed_branch_leaf}"),
                        },
                        capability: memstead_base::workspace::MountCapability::Write,
                        lifecycle: memstead_base::workspace::MountLifecycle::Eager,
                        cross_linkable: true,
                    };
                    let factory = engine.backend_factory();
                    let backend = factory(&mount).map_err(|e| {
                        memstead_base::EngineError::Mem(format!(
                            "reattach backend instantiate: {e}"
                        ))
                    })?;
                    // Clear the tombstone if one was present, so a
                    // future drift probe doesn't re-trigger the
                    // reattach branch.
                    if let Some(cfg) = existing_config.as_ref()
                        && cfg.unregistered_at.is_some()
                    {
                        let mut updated = cfg.clone();
                        updated.unregistered_at = None;
                        if let Ok(mut bytes) = serde_json::to_vec_pretty(&updated) {
                            bytes.push(b'\n');
                            if let Err(e) = backend.write_mem_config(&bytes) {
                                tracing::warn!(
                                    mem = %params.name,
                                    error = %e,
                                    "reattach: tombstone clear failed — \
                                     the marker survives; a re-unregister will \
                                     overwrite it",
                                );
                            }
                        }
                    }
                    let origin = memstead_base::MemOrigin::RuntimeCreated {
                        at: std::time::SystemTime::now(),
                        by_tool: "memstead_mem_create (reattach)",
                    };
                    engine.register_writable_mem(mount, backend, origin)?;
                    engine.persist_state()?;
                    // Re-derive every writable mem's incoming-edge slice
                    // so other mems' relationships pointing at the
                    // reattaching mem land in the in-memory edge index.
                    // Rebuilding only from the reattaching mem's own
                    // outgoing edges would leave `memstead_health`
                    // undercounting cross-mem edges visible via
                    // `memstead_entity` on the on-disk markdown.
                    // Reuses the existing workspace-wide reload path
                    // (`memstead_reload` no-arg) — schema rebuild + per-mem
                    // bodies + read-mem re-attach + workspace.toml
                    // re-read. Reports are dropped; the side effect is
                    // the edge index re-derivation.
                    let _ = engine.reload_each_writable_mem_reports()?;
                    let mut warnings: Vec<memstead_base::ops::WarningHint> = Vec::new();
                    if let Some(ts) = tombstone {
                        warnings.push(
                            memstead_base::ops::WarningHint::MemReattachedAfterUnregister {
                                mem: params.name.clone(),
                                unregistered_at: ts,
                            },
                        );
                    }
                    // Early return — the reattach path has no seed
                    // commit, no .memstead/config.json write. The branch
                    // tip stays as the prior session left it.
                    return Ok(MemCreateResponse {
                        name: params.name,
                        location: canonical,
                        schema_ref: canonical_schema_ref,
                        seed_commit_sha: String::new(),
                        warnings,
                    });
                }
            }
        }
    };
    // ---- Step 3: build MemConfig bytes ----
    // F1: every mem carries a populated `version` from creation
    // onward — `0.1.0` is the engine default; operators bump via
    // `memstead mem set-version` before publishing. Without this seed,
    // the export path hits the residual `MEM_CONFIG_INCOMPLETE` /
    // pre-fix `INTERNAL` collapse on the first archive attempt.
    let mem_config = memstead_schema::config::MemConfig {
        name: None,
        version: Some(semver::Version::new(0, 1, 0)),
        description: None,
        authors: None,
        schema: Some(canonical_schema_ref.clone()),
        write_guidance: params.write_guidance.clone(),
        rules: None,
        publish: None,
        language: None,
        read_mems: Default::default(),
        community: None,
        vcs: params.vcs.clone(),
        unregistered_at: None,
        sync_state: Default::default(),
        extra: Default::default(),
    };
    let config_bytes = serde_json::to_vec_pretty(&mem_config).map_err(|e| {
        memstead_base::EngineError::InvalidInput(format!("could not serialize mem config: {e}"))
    })?;

    // ---- Step 3b: pick storage variant ----
    // Workspace-shape heuristic. When
    // `<workspace_root>/mem-repo/.git/` exists, the new mem gets
    // a git-branch mount; otherwise folder. The probe is gix-free so
    // the heuristic works in lean builds (lean builds never have a
    // mem-repo, so the probe always returns false there).
    //
    // The git-branch storage requires the engine to have the full
    // backend factory installed (`engine_from_workspace_root` does
    // this at boot). When the factory is the default lean one, the
    // factory call below returns
    // [`memstead_base::workspace_store::InstantiateError::GitBranchRequiresMemRepoFeature`]
    // — wrapped as `EngineError::Mem` in the seed-commit step.
    // The branch leaf IS `params.name` — no separate composition step.
    // Hierarchical identity lives directly in the mem name.
    let branch_leaf = params.name.clone();
    let storage = if let Some(root) = workspace_root.as_ref() {
        let probe = root.join("mem-repo").join(".git");
        if probe.is_dir() {
            let gitdir = probe.canonicalize().unwrap_or(probe);
            memstead_base::workspace::MountStorage::GitBranch {
                gitdir,
                branch: format!("refs/heads/{branch_leaf}"),
            }
        } else {
            memstead_base::workspace::MountStorage::Folder {
                path: canonical.clone(),
            }
        }
    } else {
        memstead_base::workspace::MountStorage::Folder {
            path: canonical.clone(),
        }
    };
    let is_git_branch = matches!(
        storage,
        memstead_base::workspace::MountStorage::GitBranch { .. }
    );

    // ---- Step 4: write .memstead/config.json ----
    // Folder path: write the config blob to disk before instantiating
    // the backend so the post-register `read_mem_config()` sees it.
    // Git-branch path: skip the on-disk write — the per-mem config
    // travels in the workspace's `__MEMSTEAD` registry ref, not on disk.
    // The seed commit on the per-mem branch may also include a
    // `.memstead/config.json` blob for parity with folder backends; this
    // can be added later as an additive piece without changing the
    // wire shape.
    if !is_git_branch {
        std::fs::create_dir_all(&canonical).map_err(|e| {
            memstead_base::EngineError::Mem(format!("create_dir_all {}: {e}", canonical.display()))
        })?;
        let memstead_dir = canonical.join(memstead_base::MEM_META_DIR);
        std::fs::create_dir_all(&memstead_dir).map_err(|e| {
            memstead_base::EngineError::Mem(format!(
                "create_dir_all {}: {e}",
                memstead_dir.display()
            ))
        })?;
        let config_path = memstead_dir.join("config.json");
        if config_path.exists() {
            return Err(FullEngineError::ConfigAlreadyExists { path: config_path });
        }
        std::fs::write(&config_path, &config_bytes).map_err(|e| {
            memstead_base::EngineError::Mem(format!("write {}: {e}", config_path.display()))
        })?;
    }

    // ---- Step 5: instantiate backend + seed commit + register ----
    // Backend instantiation routes through `engine.backend_factory()`.
    // Full consumers install
    // `memstead_git_branch::storage::instantiate_full_backend` at boot so
    // the same call site produces git-branch backends when the mount
    // declares one (Step 3b above picks the variant by workspace
    // shape).
    //
    // Produce a real seed commit via the backend's commit method
    // before registering. Folder backends emit a synthetic id
    // (UNIX-nanos + counter, hex per the trait's contract);
    // git-branch backends produce real 40-char shas. Either way the
    // response carries a non-empty cursor.
    let mount = memstead_base::workspace::Mount {
        mem: params.name.clone(),
        schema: Some(canonical_schema_ref.clone()),
        storage,
        capability: memstead_base::workspace::MountCapability::Write,
        lifecycle: memstead_base::workspace::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let factory = engine.backend_factory();
    let backend = factory(&mount)
        .map_err(|e| memstead_base::EngineError::Mem(format!("instantiate backend: {e}")))?;
    let seed_ctx = memstead_base::vcs::CommitContext {
        actor: memstead_base::vcs::Actor::Agent,
        client: None,
        tool: Some("memstead_mem_create"),
        note: params.note.clone(),
        logical_operation_id: None,
        entity_ids: None,
    };
    // For git-branch mounts, write the per-mem config blob to the
    // workspace's `__MEMSTEAD` ref before sealing the per-mem branch.
    // Folder mounts already wrote the config to disk in Step 4;
    // calling `write_mem_config` again would be redundant for
    // folder.
    if is_git_branch {
        backend
            .write_mem_config(&config_bytes)
            .map_err(|e| memstead_base::EngineError::Mem(format!("write mem config: {e}")))?;
    }
    let seed_commit_sha = backend
        .commit(&format!("memstead: create mem {}", params.name), &seed_ctx)
        .map_err(|e| memstead_base::EngineError::Mem(format!("seed commit: {e}")))?;
    let origin = memstead_base::MemOrigin::RuntimeCreated {
        at: std::time::SystemTime::now(),
        by_tool: "memstead_mem_create",
    };
    // Hierarchical identity lives in `mount.mem` directly — there is
    // exactly one identifier (the full path), with no separate
    // `params.path` plumbed into the router.
    engine.register_writable_mem(mount, backend, origin)?;

    // Persist the updated mount list to the workspace store. Without
    // this, the per-mem branch + `__MEMSTEAD` config (or folder +
    // `.memstead/config.json`) lives on disk but the next CLI / MCP
    // process boots with an empty mount manifest — `unknown mem`
    // on every follow-up call. Engine-side rather than orchestrator-
    // side so every caller (MCP, UniFFI, in-process embedding)
    // inherits persistence by construction.
    engine.persist_state()?;

    // `require_notes` provenance nudge — inherited from the engine's
    // single enforcement point so mem lifecycle matches entity
    // mutations (no second, drift-prone implementation on the MCP
    // transport). The seed commit landed above; a noteless create
    // surfaces the warning without blocking.
    let note_warning = engine.note_missing_warning("create_mem", params.note.as_deref());
    Ok(MemCreateResponse {
        name: params.name,
        location: canonical,
        schema_ref: canonical_schema_ref,
        seed_commit_sha,
        warnings: note_warning.into_iter().collect(),
    })
}

/// Canonicalize a path that may or may not yet exist. Walks up
/// until the first existing ancestor, canonicalizes that, and
/// appends the tail — preserving the original segment order. Falls
/// back to the input when every ancestor is unavailable.
///
/// Mirrors full's `canonicalize_maybe_missing` in
/// `memstead_git_branch::mem_management::create`. Lifted into memstead-engine
/// so the unified create orchestrator doesn't reach back to
/// memstead-git-branch.
fn canonicalize_maybe_missing(path: &std::path::Path) -> std::path::PathBuf {
    if let Ok(c) = path.canonicalize() {
        return c;
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor: &std::path::Path = path;
    loop {
        if let Ok(c) = cursor.canonicalize() {
            let mut out = c;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            return out;
        }
        match cursor.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                match cursor.parent() {
                    Some(parent) => cursor = parent,
                    None => return path.to_path_buf(),
                }
            }
            None => return path.to_path_buf(),
        }
    }
}
