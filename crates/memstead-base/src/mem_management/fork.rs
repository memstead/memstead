//! `memstead mem fork`: a new git-branch mem whose branch starts at a
//! source mem's commit (a recorded ancestor), whose config is the
//! source's with the origin written into it, and which carries the
//! source's outgoing cross-link grants so its copied edges read
//! conformant from the first read.
//!
//! Two forms, one pipeline. **Local**: the source is a mounted
//! git-branch mem of the workspace's mem-repo; the fork's branch is a
//! ref created at the source's tip (or at a given sha the source branch
//! reaches). **Remote** (`remote: Some`): the source branch and the
//! remote's `__MEMSTEAD` ref are fetched into remote-tracking refs, the
//! source's config is read from that tracking ref (the local
//! `__MEMSTEAD` is never moved), the fetched tree is validated against
//! the source's schema pin exactly as `pull` validates before it moves a
//! pointer, and the fork's branch is created at the fetched commit.
//!
//! The fork is an engine act end to end: the branch through the
//! engine's git layer ([`crate::GitBranchOps`]), the fork commit through
//! the fork's own backend, the config through the backend's config
//! writer on the `__MEMSTEAD` ref, the grants through the workspace
//! policy writer the grant verb uses, the mount through
//! [`crate::Engine::register_writable_mem`]. Every refusal is typed and
//! lands nothing; a failure after the branch exists rolls the branch
//! (the fork commit with it) and the config back through the residue
//! prune, and the policy line through the deletion scrub (both
//! idempotent).
//!
//! **The fork commit.** A copied tree still names the source: the
//! anchors sidecar keys its rows by `<source>--<slug>`, the derivations
//! sidecar keys its baselines the same way, and a body link the source
//! qualified with its own name (`[[<source>--slug]]`, `[[<source>:slug]]`)
//! would parse in the fork as a cross-mem link into the source. So the
//! fork's first act on its branch, before the mount exists, is one
//! commit that moves every such id and link from the source's name to
//! its own ([`retarget_fork_tree`]): the rows keep their hashes, spans
//! and observations, a link naming another mem stays, a code span is
//! never touched, and a tree with nothing to move gets the commit all
//! the same (an empty retarget is still the base). Its sha is recorded
//! as `forkedFrom.base`; the ancestor on the source stays `sha`. The
//! workspace check ledger is not consulted: a fork starts unchecked,
//! because a fork's entity is not the same entity as its source's.

use std::path::Path;

use memstead_schema::workspace_config::CrossLinkValue;

use crate::FullEngineError;
use crate::backend::BackendError;
use crate::engine::GitBranchOps;
use crate::ops::WarningHint;
use crate::vcs::CommitContext;
use crate::workspace_config_edit::{
    CrossLinkTarget, grant_cross_link, scrub_policy_for_deleted_mem,
};

use super::lifecycle::{
    NOTE_MAX_LEN, ResidueProbe, admit_by_create_rules, classify_invalid_mem_name,
    residue_probe_for_workspace, sibling_name_suggestion,
};

/// Parameters for [`fork_mem`]. Mirrors `memstead mem fork
/// <source>[@<sha>] <new-name> [--remote <name>]`.
#[derive(Debug, Clone)]
pub struct MemForkParams {
    /// The source mem's name. Local form: a mounted git-branch mem of
    /// the workspace's mem-repo. Remote form: the name of the branch
    /// (`refs/heads/<source>`) and of the config blob
    /// (`mems/<source>/config.json`) on the remote.
    pub source: String,
    /// The commit the fork starts at; `None` means the source branch's
    /// tip. Anything `git rev-parse` resolves (a full or abbreviated
    /// sha), and it must be reachable from the source branch.
    pub sha: Option<String>,
    /// The new mem's name: the full hierarchical identifier, under the
    /// same grammar and create rules as any created mem.
    pub name: String,
    /// The mem-repo remote to fetch the source from; `None` is the
    /// local form.
    pub remote: Option<String>,
    /// Agent-authored provenance note (≤[`NOTE_MAX_LEN`] chars).
    pub note: Option<String>,
    /// Operator posture: skips the `[[mem_management.create]]`
    /// allowlist and its schema gate, as `create_mem` does. Never a
    /// wire-shape input from agents.
    pub operator_mode: bool,
    /// Caller category for the commits' provenance trailers.
    pub actor: crate::vcs::Actor,
    /// Client identity paired with `actor`.
    pub client: Option<crate::vcs::ClientId>,
}

/// Response of [`fork_mem`]: what was created, where it came from,
/// and what it inherited.
#[derive(Debug, Clone)]
pub struct MemForkResponse {
    /// The new mem's name.
    pub name: String,
    /// The origin as written into the fork's config: source mem, the
    /// 40-hex sha the branch starts at, the remote when one was used,
    /// and the fork commit's sha as `base`.
    pub forked_from: memstead_schema::ForkedFrom,
    /// The schema pin the fork carries: the source's, copied.
    pub schema_ref: memstead_schema::SchemaRef,
    /// The fork's branch, `refs/heads/<name>`.
    pub branch_ref: String,
    /// The source's outgoing cross-link grants the fork now carries
    /// under its own name (local form; a source with no grants, and
    /// every remote fork, inherits none).
    pub inherited_grants: Option<CrossLinkValue>,
    /// Non-fatal findings (`NOTE_MISSING` under `require_notes`).
    pub warnings: Vec<WarningHint>,
}

/// Create a mem as a fork of another git-branch mem at a recorded
/// ancestor. See the module documentation for the two forms and the
/// atomicity contract.
///
/// Refusals, all before the first write: `INVALID_MEM_NAME`,
/// `INVALID_INPUT` (no mem-repo workspace; a folder, archive or
/// in-memory source; a source config without a schema), `UNKNOWN_MEM`
/// (local source not mounted), `UNKNOWN_REMOTE`, `UNKNOWN_REF` (a
/// source branch the remote lacks, a sha that does not resolve or is
/// not on the source branch), `SCHEMA_NOT_FOUND` (the source's pin
/// does not resolve in this workspace; the message names `memstead
/// schema install`), `MEM_PATH_NOT_ALLOWED`, `MEM_SCHEMA_NOT_ALLOWED`,
/// `MEM_NAME_COLLISION`, `MEM_NAME_REF_CONFLICT`,
/// `MEM_STORAGE_RESIDUE_DETECTED`, and for the remote form
/// `SCHEMA_VIOLATION_IN_FETCH` over the fetched tree.
pub fn fork_mem(
    engine: &mut crate::Engine,
    params: MemForkParams,
) -> Result<MemForkResponse, FullEngineError> {
    // ---- Step 0: input validation ----
    if let Some(note) = params.note.as_deref()
        && note.chars().count() > NOTE_MAX_LEN
    {
        return Err(crate::EngineError::InvalidInput(format!(
            "note exceeds {NOTE_MAX_LEN} characters"
        ))
        .into());
    }
    if let Some(reason) = classify_invalid_mem_name(&params.name) {
        return Err(FullEngineError::InvalidMemName {
            name: params.name.clone(),
            reason,
        });
    }
    if crate::entity::id::validate_mem_name_grammar(&params.name).is_err() {
        return Err(FullEngineError::InvalidMemName {
            name: params.name.clone(),
            reason: "invalid_char",
        });
    }
    // The source name composes a ref and a tree path in both forms:
    // it obeys the mem-name grammar too, and a bad one is the caller's
    // input, never a git error.
    if crate::entity::id::validate_mem_name_grammar(&params.source).is_err() {
        return Err(crate::EngineError::InvalidInput(format!(
            "source {:?} is not a mem name (grammar `[a-z0-9-]+(/[a-z0-9-]+)*`)",
            params.source
        ))
        .into());
    }
    if params.source == params.name {
        return Err(crate::EngineError::InvalidInput(format!(
            "a fork needs its own name: {:?} is the source",
            params.name
        ))
        .into());
    }

    // ---- Step 0b: workspace shape ----
    // A fork is a branch at an ancestor: it needs the mem-repo gitdir
    // and the git layer. A folder-only workspace has neither.
    let root = engine
        .workspace_root()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            crate::EngineError::InvalidInput(
                "mem fork requires a workspace root to locate mem-repo/.git/".to_string(),
            )
        })?;
    let gitdir = root.join("mem-repo").join(".git");
    if !gitdir.is_dir() {
        return Err(crate::EngineError::InvalidInput(format!(
            "mem fork requires a mem-repo workspace ({} not found): a fork is a branch at \
             an ancestor, and a folder-only workspace has no branch to fork",
            gitdir.display()
        ))
        .into());
    }
    let gitdir = gitdir.canonicalize().unwrap_or(gitdir);
    let ops = engine.git_branch_ops().ok_or_else(|| {
        crate::EngineError::InvalidInput(
            "mem fork requires the git-branch ops bundle (full boot only)".to_string(),
        )
    })?;
    let ctx = engine.commit_context(
        Some("memstead_mem_fork"),
        params.actor,
        params.client.clone(),
        params.note.clone(),
    );

    // ---- Step 1: the source's config bytes and the ref its tip is read from ----
    let (config_bytes, tip_ref) = match params.remote.as_deref() {
        None => resolve_local_source(engine, &gitdir, &params.source)?,
        Some(remote) => resolve_remote_source(&ops, &gitdir, remote, &params.source)?,
    };
    let source_value: serde_json::Value = serde_json::from_slice(&config_bytes).map_err(|e| {
        crate::EngineError::InvalidInput(format!(
            "source {:?}: its config is not JSON: {e}",
            params.source
        ))
    })?;
    let source_config = memstead_schema::parse_mem_config(&source_value).map_err(|e| {
        crate::EngineError::InvalidInput(format!(
            "source {:?}: its config does not parse: {e}",
            params.source
        ))
    })?;

    // ---- Step 2: the commit the fork starts at ----
    let tip = (ops.resolve_ref)(&gitdir, &tip_ref)
        .map_err(crate::EngineError::Backend)?
        .ok_or_else(|| crate::EngineError::UnknownRef(tip_ref.clone()))?;
    let sha = match params.sha.as_deref() {
        None => tip.clone(),
        Some(raw) => {
            // `^{commit}`: the object must exist and peel to a commit.
            // A bare `rev-parse --verify` accepts any 40-hex string
            // whether or not the store holds it, and the ancestry
            // check would then fail raw.
            let full = (ops.resolve_ref)(&gitdir, &format!("{raw}^{{commit}}"))
                .map_err(crate::EngineError::Backend)?
                .ok_or_else(|| {
                    crate::EngineError::UnknownRef(format!(
                        "{raw} is not a commit in the mem-repo (source branch {tip_ref})"
                    ))
                })?;
            let on_branch = (ops.is_ancestor)(&gitdir, &full, &tip).map_err(|e| {
                crate::EngineError::UnknownRef(format!(
                    "{raw} could not be placed against {tip_ref}: {e}"
                ))
            })?;
            if !on_branch {
                return Err(crate::EngineError::UnknownRef(format!(
                    "{raw} is not on {tip_ref} (tip {tip}): a fork starts at a commit the \
                     source branch reaches"
                ))
                .into());
            }
            full
        }
    };

    // ---- Step 3: the schema pin, copied and resolved, never re-pinned ----
    let pin = source_config.schema.clone().ok_or_else(|| {
        crate::EngineError::InvalidInput(format!(
            "source {:?}: its config declares no schema pin",
            params.source
        ))
    })?;
    let mut catalogue: Vec<std::sync::Arc<memstead_schema::Schema>> =
        engine.workspace_schemas().to_vec();
    catalogue.extend_from_slice(engine.builtin_schemas());
    let resolved_schema = crate::engine::SchemaResolver::new(&catalogue)
        .resolve(&pin)
        .map_err(|sources| {
            // The pin is copied and never re-pinned, so the remedy is
            // always the install, never `mem set-schema` (which would
            // name a mem that does not exist yet): a package of the
            // name in the tree when there is one, the placeholder
            // otherwise.
            let hint = crate::engine::error::probe_authoring_package(&root, &pin.name)
                .unwrap_or_else(|| {
                    crate::engine::error::SCHEMA_INSTALL_PACKAGE_UNKNOWN.to_string()
                });
            crate::EngineError::SchemaNotFound {
                mem: params.name.clone(),
                pin: pin.to_string(),
                sources,
                install_hint: Some(hint),
            }
        })?;
    let canonical_schema_ref = memstead_schema::SchemaRef::new(
        resolved_schema.manifest.name.clone(),
        resolved_schema.version.clone(),
    );

    // ---- Step 4: the create rules, as for any created mem ----
    let canonical_location = root.join(&params.name);
    if !params.operator_mode {
        admit_by_create_rules(
            engine,
            &params.name,
            &canonical_location,
            &canonical_schema_ref,
            &catalogue,
        )?;
    }

    // ---- Step 5: collisions, all before any write ----
    if let Some(existing) = engine.mem_router().origin_for_mem(&params.name) {
        return Err(crate::EngineError::MemNameCollision {
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
        return Err(crate::EngineError::MemNameCollision {
            name: params.name,
            source_origin: "attached read mem".to_string(),
        }
        .into());
    }
    let conflicting = (ops.branch_namespace_conflicts)(&gitdir, &params.name)
        .map_err(|e| crate::EngineError::Mem(format!("ref namespace probe: {e}")))?;
    if !conflicting.is_empty() {
        let suggestion = sibling_name_suggestion(&params.name, &conflicting);
        return Err(FullEngineError::MemNameRefConflict {
            branch_ref: format!("refs/heads/{}", params.name),
            name: params.name,
            conflicting_branches: conflicting,
            suggestion,
        });
    }
    // Residue at the name (an unregistered mem's branch and config, a
    // crashed create) refuses outright: a fork offers no reattach and
    // no overwrite, because the branch it would create is the lineage
    // claim and must not adopt or destroy another one.
    if let ResidueProbe::Present {
        branch_ref,
        config_blob,
        ..
    } = residue_probe_for_workspace(
        engine,
        Some(&root),
        &params.name,
        &params.name,
        &canonical_schema_ref,
    ) {
        return Err(FullEngineError::MemStorageResidueDetected {
            branch_ref,
            config_blob,
            entity_count: 0,
        });
    }

    // ---- Step 6: remote form: the fetched tree passes the schema before anything lands ----
    // The same check `pull` runs before it moves a pointer, against the
    // schema the source's pin resolved to. The local form skips it:
    // the source's tree is already this engine's validated state.
    if params.remote.is_some() {
        crate::Engine::validate_ref_with_schema(
            &ops,
            &gitdir,
            &params.name,
            &sha,
            resolved_schema.as_ref(),
        )?;
    }

    // ---- Step 7: the fork's config, short of its origin ----
    // The source's config with the source's own cursors left behind:
    // sync state and the review mark are the source's maintenance
    // bookkeeping, the tombstone is not the fork's, and the name is
    // path-derived. The origin is written in once the fork commit
    // exists, because it records that commit's sha.
    let mut config = source_config;
    config.name = None;
    config.sync_state.clear();
    config.review_mark = None;
    config.unregistered_at = None;

    // ---- Step 8: the writes, each rolled back on the next one's failure ----
    let branch_ref = format!("refs/heads/{}", params.name);
    (ops.create_branch_at)(&gitdir, &params.name, &sha)
        .map_err(|e| crate::EngineError::Mem(format!("fork branch: {e}")))?;
    let mount = crate::workspace::Mount {
        mem: params.name.clone(),
        schema: Some(canonical_schema_ref.clone()),
        storage: crate::workspace::MountStorage::GitBranch {
            gitdir: gitdir.clone(),
            branch: branch_ref.clone(),
        },
        capability: crate::workspace::MountCapability::Write,
        lifecycle: crate::workspace::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let factory = engine.backend_factory();
    let backend = match factory(&mount) {
        Ok(b) => b,
        Err(e) => {
            roll_back(
                &ops,
                &gitdir,
                &root,
                &params.name,
                &ctx,
                "backend instantiate",
            );
            return Err(crate::EngineError::Mem(format!("instantiate backend: {e}")).into());
        }
    };
    // The fork commit: sidecar ids and self-links move to the fork's
    // name in one commit on the fork's branch, right above the
    // ancestor. The branch rollback covers it (the prune drops the ref,
    // and the commit with it).
    let base = match retarget_fork_tree(backend.as_ref(), &params.source, &params.name, &sha, &ctx)
    {
        Ok(base) => base,
        Err(e) => {
            roll_back(&ops, &gitdir, &root, &params.name, &ctx, "fork commit");
            return Err(e.into());
        }
    };
    let forked_from = memstead_schema::ForkedFrom {
        mem: params.source.clone(),
        sha: sha.clone(),
        remote: params.remote.clone(),
        base: Some(base),
    };
    config.forked_from = Some(forked_from.clone());
    let config_bytes = match serde_json::to_vec_pretty(&config) {
        Ok(mut bytes) => {
            bytes.push(b'\n');
            bytes
        }
        Err(e) => {
            roll_back(&ops, &gitdir, &root, &params.name, &ctx, "config serialize");
            return Err(crate::EngineError::InvalidInput(format!(
                "could not serialize the fork's config: {e}"
            ))
            .into());
        }
    };
    if let Err(e) = backend.write_mem_config(&config_bytes, &ctx) {
        roll_back(&ops, &gitdir, &root, &params.name, &ctx, "config write");
        return Err(crate::EngineError::Mem(format!("write mem config: {e}")).into());
    }

    // The source's outgoing grants, under the fork's name, through the
    // policy writer the grant verb uses. Local form only: a remote's
    // policy is not this workspace's, and the fork gets what any
    // created mem gets there (the create rule's `default_cross_links`,
    // synthesised at link time).
    let inherited_grants: Option<CrossLinkValue> = if params.remote.is_none() {
        match engine.settings().cross_mem_links.get(&params.source) {
            Some(CrossLinkValue::List(targets)) if targets.is_empty() => None,
            other => other.cloned(),
        }
    } else {
        None
    };
    if let Some(value) = &inherited_grants
        && let Err(e) = write_inherited_grants(engine, &root, &params.name, value)
    {
        roll_back(
            &ops,
            &gitdir,
            &root,
            &params.name,
            &ctx,
            "grant inheritance",
        );
        return Err(crate::EngineError::Mem(format!("inherit cross-link grants: {e}")).into());
    }

    let origin = crate::MemOrigin::RuntimeCreated {
        at: std::time::SystemTime::now(),
        by_tool: "memstead_mem_fork",
    };
    if let Err(e) = engine.register_writable_mem(mount, backend, origin) {
        roll_back(
            &ops,
            &gitdir,
            &root,
            &params.name,
            &ctx,
            "mount registration",
        );
        return Err(e.into());
    }
    if let Some(value) = &inherited_grants {
        // The writer changed the file; the running engine's policy
        // follows it, so the fork's edges read conformant in this
        // process too, not only after the next boot.
        let mut settings = engine.settings().clone();
        settings
            .cross_mem_links
            .insert(params.name.clone(), value.clone());
        engine.set_settings(settings);
    }
    engine.persist_state()?;

    let warnings: Vec<WarningHint> = engine
        .note_missing_warning("fork_mem", params.note.as_deref())
        .into_iter()
        .collect();
    Ok(MemForkResponse {
        name: params.name,
        forked_from,
        schema_ref: canonical_schema_ref,
        branch_ref,
        inherited_grants,
        warnings,
    })
}

/// The fork commit: move every id and link that names `source` to
/// `fork`, in the anchors sidecar (entity keys, an entity-grain row's
/// `artifact`, every `derived_from` entry naming the source mem), the
/// derivations sidecar (source keys and same-mem targets) and the entity bodies
/// (mem-qualified self-links, through the export retargeting rule:
/// `[[<source>--slug]]` and `[[<source>:slug]]`, labels kept, code
/// spans and links naming another mem untouched), then commit once on
/// the fork's branch with the caller's provenance. Rows keep their
/// hashes, spans and observations; a body with nothing to move keeps
/// its bytes. The commit is made even when nothing moved, so every
/// fork has a base of its own above the ancestor. Returns the commit's
/// sha.
fn retarget_fork_tree(
    backend: &dyn crate::backend::MemBackend,
    source: &str,
    fork: &str,
    ancestor: &str,
    ctx: &CommitContext<'_>,
) -> Result<String, crate::EngineError> {
    // The anchors sidecar: entity keys.
    if let Some(bytes) = backend.read_anchors_sidecar()? {
        let mut sidecar = crate::anchor::AnchorSidecar::from_bytes(&bytes).map_err(|e| {
            crate::EngineError::Mem(format!(
                "fork {fork:?}: the source's anchors sidecar does not parse: {e}"
            ))
        })?;
        let mut moved = false;
        // A row that names a same-mem entity as its artifact (entity
        // grain) or among its inputs would otherwise resolve against
        // the source's entity from the fork, and read unobserved where
        // the source is not mounted.
        for row in sidecar.entities.values_mut().flatten() {
            if row.grain == crate::anchor::AnchorGrain::Entity
                && let Some(to) = retarget_entity_id(&row.artifact, source, fork)
            {
                row.artifact = to;
                moved = true;
            }
            for input in &mut row.derived_from {
                if let Some(to) = retarget_entity_id(input, source, fork) {
                    *input = to;
                    moved = true;
                }
            }
        }
        let keys: Vec<String> = sidecar.entities.keys().cloned().collect();
        for key in keys {
            if let Some(to) = retarget_entity_id(&key, source, fork) {
                sidecar.rename(&key, &to);
                moved = true;
            }
        }
        if moved {
            backend.write_anchors_sidecar(&sidecar.to_bytes())?;
        }
    }

    // The derivations sidecar: source keys and same-mem targets.
    let derivations_path = Path::new(crate::derivation::DERIVATION_SIDECAR_PATH);
    if let Some(bytes) = backend.read_entity(derivations_path)? {
        let sidecar = crate::derivation::DerivationSidecar::from_bytes(&bytes).map_err(|e| {
            crate::EngineError::Mem(format!(
                "fork {fork:?}: the source's derivations sidecar does not parse: {e}"
            ))
        })?;
        let mut moved = false;
        let mut retargeted = crate::derivation::DerivationSidecar {
            version: sidecar.version,
            baselines: Default::default(),
        };
        for (key, baselines) in sidecar.baselines {
            let key = match retarget_entity_id(&key, source, fork) {
                Some(to) => {
                    moved = true;
                    to
                }
                None => key,
            };
            let baselines = baselines
                .into_iter()
                .map(|mut b| {
                    if let Some(to) = retarget_entity_id(&b.target, source, fork) {
                        b.target = to;
                        moved = true;
                    }
                    b
                })
                .collect();
            retargeted.baselines.insert(key, baselines);
        }
        if moved {
            backend.write_entity(derivations_path, &retargeted.to_bytes())?;
        }
    }

    // The entity bodies: mem-qualified self-links.
    for rel_path in backend.list_entities()? {
        let Some(bytes) = backend.read_entity(&rel_path)? else {
            continue;
        };
        let retargeted = crate::ops::export::retarget_mem_links(bytes.clone(), source, fork);
        if retargeted != bytes {
            backend.write_entity(&rel_path, &retargeted)?;
        }
    }

    let short = &ancestor[..ancestor.len().min(12)];
    let subject = format!("memstead: fork mem {fork} from {source}@{short}");
    Ok(backend.commit(&subject, ctx)?)
}

/// `<source>--<slug>` as `<fork>--<slug>`; `None` for an id qualified
/// by any other mem (a cross-mem target, a foreign row), which the fork
/// leaves as it found it.
fn retarget_entity_id(id: &str, source: &str, fork: &str) -> Option<String> {
    let slug = id.strip_prefix(source)?.strip_prefix("--")?;
    (!slug.is_empty()).then(|| format!("{fork}--{slug}"))
}

/// Local form: the source is a mounted git-branch mem of this
/// mem-repo. Returns its config bytes (read through its own backend,
/// the freshest state) and the ref its tip is read from.
fn resolve_local_source(
    engine: &crate::Engine,
    gitdir: &Path,
    source: &str,
) -> Result<(Vec<u8>, String), FullEngineError> {
    let mount = engine
        .mount(source)
        .cloned()
        .ok_or_else(|| engine.unknown_mem_error(source))?;
    let branch = match &mount.storage {
        crate::workspace::MountStorage::GitBranch {
            gitdir: source_gitdir,
            branch,
        } => {
            let source_gitdir = source_gitdir
                .canonicalize()
                .unwrap_or(source_gitdir.clone());
            if source_gitdir != gitdir {
                return Err(crate::EngineError::InvalidInput(format!(
                    "source {source:?} lives in {} and not in the workspace mem-repo {}: a fork \
                     is a branch of the same mem-repo",
                    source_gitdir.display(),
                    gitdir.display()
                ))
                .into());
            }
            crate::workspace::branch_full_ref(branch)
        }
        crate::workspace::MountStorage::Folder { .. } => {
            return Err(crate::EngineError::InvalidInput(format!(
                "source {source:?} is a folder mem: a fork needs a git-branch source, because \
                 the branch is the lineage"
            ))
            .into());
        }
        crate::workspace::MountStorage::Archive { .. } => {
            return Err(crate::EngineError::InvalidInput(format!(
                "source {source:?} is a sealed archive mount: a fork needs a git-branch source, \
                 because the branch is the lineage"
            ))
            .into());
        }
        crate::workspace::MountStorage::InMemory => {
            return Err(crate::EngineError::InvalidInput(format!(
                "source {source:?} is an in-memory mem: a fork needs a git-branch source, \
                 because the branch is the lineage"
            ))
            .into());
        }
    };
    let factory = engine.backend_factory();
    let backend = factory(&mount)
        .map_err(|e| crate::EngineError::Mem(format!("source backend instantiate: {e}")))?;
    let bytes = backend
        .read_mem_config()
        .map_err(crate::EngineError::Backend)?
        .ok_or_else(|| {
            crate::EngineError::Mem(format!(
                "source {source:?}: no config on __MEMSTEAD:mems/{source}/config.json"
            ))
        })?;
    Ok((bytes, branch))
}

/// Remote form: the remote's branch roster first (typed
/// `UNKNOWN_REMOTE`, and a branch the remote lacks refuses before a
/// fetch), then one fetch of the source branch and the remote's
/// `__MEMSTEAD` into remote-tracking refs. The local `__MEMSTEAD` is
/// never moved. Returns the source's config bytes from the tracking
/// ref and the tracking ref of the source branch.
fn resolve_remote_source(
    ops: &GitBranchOps,
    gitdir: &Path,
    remote: &str,
    source: &str,
) -> Result<(Vec<u8>, String), FullEngineError> {
    let heads = (ops.ls_remote)(gitdir, remote).map_err(lift_remote_marker)?;
    let source_ref = format!("refs/heads/{source}");
    if !heads.iter().any(|(name, _)| name == &source_ref) {
        return Err(crate::EngineError::UnknownRef(format!(
            "remote {remote:?} has no branch {source_ref}: the source mem {source:?} is not on \
             that remote"
        ))
        .into());
    }
    if !heads
        .iter()
        .any(|(name, _)| name == "refs/heads/__MEMSTEAD")
    {
        return Err(crate::EngineError::InvalidInput(format!(
            "remote {remote:?} carries no __MEMSTEAD ref: it is not a mem-repo, so there is no \
             config to fork"
        ))
        .into());
    }
    let tracking_source = format!("refs/remotes/{remote}/{source}");
    let tracking_registry = format!("refs/remotes/{remote}/__MEMSTEAD");
    (ops.fetch)(
        gitdir,
        remote,
        &[
            format!("+{source_ref}:{tracking_source}"),
            format!("+refs/heads/__MEMSTEAD:{tracking_registry}"),
        ],
    )
    .map_err(lift_remote_marker)?;
    let bytes = (ops.read_config_at_ref)(gitdir, &tracking_registry, source)
        .map_err(crate::EngineError::Backend)?
        .ok_or_else(|| {
            crate::EngineError::InvalidInput(format!(
                "remote {remote:?}: its __MEMSTEAD carries no config for {source:?} \
                 (mems/{source}/config.json)"
            ))
        })?;
    Ok((bytes, tracking_source))
}

/// The transport's in-band remote marker, lifted to the typed code the
/// other transport verbs use.
fn lift_remote_marker(e: BackendError) -> FullEngineError {
    match e {
        BackendError::Other(msg) if msg.starts_with("UNKNOWN_REMOTE:") => {
            crate::EngineError::UnknownRemote(
                msg.trim_start_matches("UNKNOWN_REMOTE:").trim().to_string(),
            )
            .into()
        }
        other => crate::EngineError::Backend(other).into(),
    }
}

/// The source's grant value, written under the fork's name through the
/// same writer `workspace grant-cross-link` uses: the wildcard as one
/// grant, a list as one grant per target.
fn write_inherited_grants(
    engine: &crate::Engine,
    root: &Path,
    name: &str,
    value: &CrossLinkValue,
) -> Result<(), crate::workspace_config_edit::WorkspaceEditError> {
    let known: Vec<String> = engine.mem_names().into_iter().map(str::to_string).collect();
    match value {
        CrossLinkValue::Wildcard => {
            grant_cross_link(root, name, &CrossLinkTarget::Wildcard, &known)?;
        }
        CrossLinkValue::List(targets) => {
            for target in targets {
                grant_cross_link(root, name, &CrossLinkTarget::Named(target.clone()), &known)?;
            }
        }
    }
    Ok(())
}

/// Undo a partial fork: the branch and the config blob through the
/// residue prune, the policy line through the deletion scrub. Both are
/// idempotent, so a stage that never wrote is not an error. A failing
/// rollback is logged with the manual remedy; the caller still returns
/// the original refusal.
fn roll_back(
    ops: &GitBranchOps,
    gitdir: &Path,
    root: &Path,
    name: &str,
    ctx: &CommitContext<'_>,
    stage: &str,
) {
    if let Err(e) = (ops.prune_residue)(gitdir, name, ctx) {
        tracing::warn!(
            mem = %name,
            stage,
            error = %e,
            "fork_mem: {stage} failed and the branch rollback failed too; \
             `memstead mem delete <name> --operator-mode` removes the leftover"
        );
    }
    if let Err(e) = scrub_policy_for_deleted_mem(root, name) {
        tracing::warn!(
            mem = %name,
            stage,
            error = %e,
            "fork_mem: {stage} failed and the policy rollback failed too; \
             `memstead workspace revoke-cross-link <name> <target>` removes the leftover"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::retarget_entity_id;

    /// The id rule the two sidecars share: exactly the source's own
    /// ids move, by exact qualifier match; hierarchical names included.
    #[test]
    fn retarget_entity_id_moves_the_sources_ids_and_nothing_else() {
        assert_eq!(
            retarget_entity_id("specs--alpha", "specs", "specs-fork").as_deref(),
            Some("specs-fork--alpha")
        );
        assert_eq!(
            retarget_entity_id(
                "stocks/ai-citations--claim-one",
                "stocks/ai-citations",
                "proposals/ai-citations-001"
            )
            .as_deref(),
            Some("proposals/ai-citations-001--claim-one")
        );
        // A slug with dashes keeps them.
        assert_eq!(
            retarget_entity_id("specs--a--b", "specs", "f").as_deref(),
            Some("f--a--b")
        );
        // Another mem's id, a mem whose name merely starts with the
        // source's, a bare slug, an empty slug: untouched.
        assert_eq!(retarget_entity_id("plans--roadmap", "specs", "f"), None);
        assert_eq!(retarget_entity_id("specs-old--alpha", "specs", "f"), None);
        assert_eq!(retarget_entity_id("alpha", "specs", "f"), None);
        assert_eq!(retarget_entity_id("specs--", "specs", "f"), None);
    }
}
