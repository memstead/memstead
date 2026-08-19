//! Below-boot repair surface — the verbs a boot-failure message names
//! must run on exactly the workspace whose boot they repair.
//!
//! During the 2026-08-06/07 plenum outage both named remedies
//! (`memstead schema install`, `memstead mem set-schema`) booted the
//! full workspace unconditionally, so they failed on the very boot they
//! were supposed to fix. This module supplies their below-boot forms:
//! they load the workspace *description* (mount roster) and touch only
//! configuration and schema storage — no backend-wide instantiation, no
//! schema-pin resolution over every mem, no entity load. "Below boot"
//! means below *workspace load*, never outside the engine's write
//! discipline: pin writes go through the same
//! [`memstead_base::engine::lifecycle::bump_backend_schema_pin`] the
//! booted path uses, target refs resolve through the same
//! [`SchemaResolver`] over the same catalogue construction, and package
//! validation is the same [`Engine::validate_schema_package`] gate —
//! one implementation per check, so the booted and below-boot paths
//! cannot fork into two validation regimes.
//!
//! What below-boot `set-schema` deliberately does NOT do: the booted
//! path's conformance gate over loaded entities (migration semantics).
//! Entities are unreadable before boot — that is the point. The pin is
//! switched directly; the next (now green) boot's health surfaces any
//! conformance findings.

use std::path::Path;
use std::sync::Arc;

use memstead_base::engine::error::SchemaSourceDiagnostic;
use memstead_base::engine::lifecycle::bump_backend_schema_pin;
use memstead_base::engine::{SchemaResolver, load_workspace_schemas};
use memstead_base::workspace_store::WorkspaceStoreAdapter;
use memstead_base::{BootError, Engine, EngineError, FileWorkspaceStore};

/// Outcome of [`set_mem_schema_below_boot`].
#[derive(Debug, serde::Serialize)]
pub struct BelowBootSetSchema {
    pub mem: String,
    /// The new settled pin, `<name>@<version>`.
    pub schema_pin: String,
    /// Whether the mem's backend config carried the pin (config-absent
    /// mems keep `Mount.schema` in `mounts.json` as their settled pin).
    pub config_updated: bool,
    /// Always `false` on this path — recorded explicitly so consumers
    /// (and the operator) see that the booted path's conformance gate
    /// did not run; the next boot's health carries any findings.
    pub conformance_checked: bool,
}

/// The schema-resolution catalogue a below-boot repair consults —
/// the same construction the boot path performs in
/// `engine_from_workspace_root`: workspace-authored schemas (the fixed
/// `.memstead/schemas/` dir via the shared [`load_workspace_schemas`]
/// walker, plus the `__MEMSTEAD:schemas/` ref) layered over the
/// built-ins. Ref-read failures degrade to built-ins with a warning,
/// mirroring the boot path's best-effort overlay.
fn below_boot_schema_catalogue(
    workspace_root: &Path,
) -> Result<Vec<Arc<memstead_schema::Schema>>, EngineError> {
    let fixed_dir = workspace_root.join(".memstead").join("schemas");
    let mut catalogue = load_workspace_schemas(Some(fixed_dir.as_path()))?;
    use memstead_base::schema_source::SchemaSource as _;
    match crate::mem_repo_schemas::GitBranchSchemaSource::for_workspace(workspace_root)
        .read_schemas()
    {
        Ok(schemas) => catalogue.extend(schemas),
        Err(e) => {
            tracing::warn!(
                "below-boot repair: could not read schemas from `__MEMSTEAD:schemas/` at {}: {e}; \
                 resolving against the folder dir and built-ins only",
                workspace_root.display()
            );
        }
    }
    catalogue.extend(
        memstead_schema::builtins::load_builtin_schemas()
            .map_err(|e| EngineError::SchemaResolverInit(e.to_string()))?,
    );
    Ok(catalogue)
}

/// Repin a mem's schema without booting the workspace — the below-boot
/// form of `memstead mem set-schema`, for workspaces whose boot fails
/// (typically on the unresolvable pin this call repairs).
///
/// The target ref must resolve in the shared catalogue; an unresolvable
/// target refuses with the same `SCHEMA_NOT_FOUND` trail the booted
/// path produces — repair never force-writes a pin that resolves
/// nowhere. A corrupt workspace store refuses typed through
/// [`BootError::Store`].
pub fn set_mem_schema_below_boot(
    workspace_root: &Path,
    mem: &str,
    target: &memstead_schema::SchemaRef,
) -> Result<BelowBootSetSchema, BootError> {
    let mut workspace = crate::workspace_store::load_workspace_description(workspace_root)?;
    let mount_idx = workspace
        .mounts
        .iter()
        .position(|m| m.mem == mem)
        .ok_or_else(|| BootError::Engine(EngineError::UnknownMem(mem.to_string())))?;

    // Shared target-ref validation: same resolver, same catalogue
    // construction, same refusal shape as the booted path.
    let catalogue = below_boot_schema_catalogue(workspace_root).map_err(BootError::Engine)?;
    SchemaResolver::new(&catalogue)
        .resolve(target)
        .map_err(|_sources| {
            BootError::Engine(
                EngineError::SchemaNotFound {
                    mem: mem.to_string(),
                    pin: target.as_display(),
                    sources: SchemaSourceDiagnostic::for_failed_pin(
                        &target.name,
                        &target.version,
                        &catalogue,
                    ),
                    install_hint: None,
                }
                .with_schema_install_probe(Some(workspace_root)),
            )
        })?;

    // Authoritative home first: the mem's backend config, through the
    // same value-level bump the booted path uses. Only this one mount's
    // backend is instantiated — no workspace-wide boot.
    let backend = crate::storage::instantiate_full_backend(&workspace.mounts[mount_idx])
        .map_err(|e| BootError::Engine(EngineError::Mem(e.to_string())))?;
    let config_updated = bump_backend_schema_pin(backend.as_ref(), target)
        .map_err(BootError::Engine)?
        .is_some();

    // Keep the mounts.json assertion in sync and clear any in-flight
    // migration target (the repair settles the pin). Standalone
    // workspaces (bare folder mem, no `.memstead/workspace.toml`
    // marker) have no mount state to persist — the config bump above
    // is their whole repair.
    workspace.mounts[mount_idx].schema = Some(target.clone());
    workspace.mounts[mount_idx].migration_target = None;
    if matches!(
        memstead_base::detect_layout(workspace_root),
        memstead_base::Layout::New
    ) {
        FileWorkspaceStore::new().save_state(workspace_root, &workspace)?;
    }

    Ok(BelowBootSetSchema {
        mem: mem.to_string(),
        schema_pin: target.as_display(),
        config_updated,
        conformance_checked: false,
    })
}

/// Install a schema package onto the workspace's `__MEMSTEAD:schemas/`
/// ref without booting the workspace — the below-boot form of
/// `memstead schema install` for mem-repo workspaces. Runs the same
/// [`Engine::validate_schema_package`] gate as the booted path, then
/// writes through the same ref writer. Returns the resulting
/// `__MEMSTEAD` tip commit sha.
///
/// The gitdir resolves like the booted path prefers it: a git-branch
/// mount's declared gitdir from the workspace description, falling
/// back to `<root>/mem-repo/.git` when no git-branch mount is
/// declared. A genuinely corrupt workspace store refuses typed
/// (`BootError::Store`) — repair operates below boot, not below the
/// workspace's own description.
pub fn install_schema_below_boot(
    workspace_root: &Path,
    name: &str,
    version: &str,
    files: &[(String, Vec<u8>)],
) -> Result<String, BootError> {
    Engine::validate_schema_package(name, version, files).map_err(BootError::Engine)?;
    let gitdir = crate::workspace_store::load_workspace_description(workspace_root)?
        .mounts
        .iter()
        .find_map(|m| match &m.storage {
            memstead_base::MountStorage::GitBranch { gitdir, .. } => Some(gitdir.clone()),
            _ => None,
        })
        .unwrap_or_else(|| workspace_root.join("mem-repo").join(".git"));
    if !gitdir.exists() {
        return Err(BootError::Engine(EngineError::Mem(format!(
            "schema install requires a mem-repo workspace — no git-branch mount and no \
             mem-repo gitdir at {}",
            gitdir.display()
        ))));
    }
    // Seal AS-GIVEN — the resolver decides the generation (see
    // `with_format_marker`'s contract); this seam never invents one.
    let outcome =
        crate::storage_memstead::write_schema_to_memstead_ref(&gitdir, name, version, files)
            .map_err(|e| {
                BootError::Engine(EngineError::Mem(format!(
                    "schema install onto `__MEMSTEAD:schemas/{name}@{version}` failed: {e}"
                )))
            })?;
    Ok(outcome.commit_sha)
}
