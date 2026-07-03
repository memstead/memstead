//! Persistence adapter — reads / writes the [`Workspace`] from disk
//! (or other backing stores) so [`crate::Engine::from_mounts`] can
//! consume an in-memory mount list without owning the disk format.
//!
//! ## Two-layer file adapter (default)
//!
//! [`FileWorkspaceStore`] is the default adapter. It splits the
//! workspace's persisted state across two files under
//! `<workspace>/.memstead/`:
//!
//! - `workspace.toml` — operator-edited. Carries the persistence-
//!   adapter declaration plus (in later sessions) cross-mem
//!   permissions, workspace-level policy, plugin hooks. The engine
//!   never writes to this file.
//! - `state/mounts.json` — engine-managed. Carries the mount list
//!   (per-mount mem name, schema pin, capability, lifecycle,
//!   cross-linkable flag, and the backend-specific storage reference
//!   — folder path, gitdir+branch pair, or archive path). The
//!   operator does not edit this file during normal operation.
//!
//! The split mirrors the natural authorship: the operator edits rules
//! that change rarely; the engine writes mount-list entries that
//! change often (planning mems, ingest scratch mems, …). Sharing
//! one file would force two authors with different update frequencies
//! through the same merge surface.
//!
//! ## Adapter trait
//!
//! [`WorkspaceStoreAdapter`] is the seam. Future adapters (SQLite,
//! remote, in-memory test fixture) implement it without changing the
//! engine API.
//! The adapter is selected at startup via the persistence-adapter
//! declaration in `workspace.toml` — the file adapter is the only
//! built-in V1.
//!
//! ## Backend instantiation
//!
//! The adapter produces a [`Workspace`] (mount list + operator
//! policy). Turning each [`Mount`]'s [`MountStorage`] into a
//! `Box<dyn MemBackend>` is a separate concern — handled by
//! [`instantiate_lean_backend`] for folder + archive variants. The
//! git-branch backend lives in the `memstead-git-branch` crate behind the
//! `mem-repo` Cargo feature; consumers in the lean flavour cannot
//! materialise a `MountStorage::GitBranch` mount and surface
//! [`InstantiateError::GitBranchRequiresMemRepoFeature`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backend::MemBackend;
use crate::storage::{ArchiveBackend, FilesystemMemWriter, InMemoryBackend};
use crate::workspace::{
    McpSection, Mount, MountCapability, MountLifecycle, MountStorage, MutationsSection, Workspace,
    WorkspaceSettings,
};

/// The engine-managed workspace store directory under the workspace
/// root — `<workspace_root>/.memstead/` holds `workspace.toml`,
/// `state/mounts.json`, and the tier-3 install cache. Distinct from
/// the per-mem meta directory
/// ([`memstead_schema::MEM_META_DIR`], re-exported as
/// `crate::mem::MEM_META_DIR`) and from the literal `".memstead/..."`
/// member paths inside sealed archives, which are a separate on-disk
/// format and never use this constant.
pub const WORKSPACE_STORE_DIR: &str = ".memstead";

/// Errors surfaced by [`WorkspaceStoreAdapter::load`] and
/// [`WorkspaceStoreAdapter::save_state`]. Backend-specific failures
/// surface as [`StoreError::Other`] with a string message; structured
/// per-adapter errors can extend the enum later.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Workspace root has no `.memstead/` directory or no recognised
    /// adapter file inside it. Distinct from `Io` so callers can
    /// distinguish "needs `memstead init`" from "permissions broke".
    #[error("workspace store not found at {path}")]
    NotInitialised { path: PathBuf },
    /// IO failure reading or writing one of the adapter files.
    #[error("workspace store io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// TOML or JSON parse / serialise failure. The wrapped string is
    /// the underlying serde error; the file is named so operators
    /// know where to look.
    #[error("workspace store parse error at {path}: {message}")]
    Parse { path: PathBuf, message: String },
    /// Format version mismatch — adapter understands a different
    /// schema version than the file declares.
    #[error("workspace store format mismatch at {path}: expected {expected}, found {found}")]
    FormatMismatch {
        path: PathBuf,
        expected: String,
        found: String,
    },
    /// Pre-rename workspace layout. The unit-noun cut renamed every
    /// on-disk shape with no dual-read; refusing keeps an old
    /// workspace from booting empty, half-mounted, or silently
    /// rewritten. The message names the one-shot migration steps.
    #[error(
        "pre-rename workspace layout at {path} (found format {found}): migrate the workspace \
         state in place — rewrite mounts.json to memstead-mounts-3 (record field `mem`, storage \
         paths under mem-repo/), workspace.toml to memstead-git-branch-2 (tables `mem_management`, \
         `cross_mem_links`), rename the gitdir container to mem-repo/, and move the metadata \
         branch tree to mems/ — then retry"
    )]
    LegacyLayout { path: PathBuf, found: String },
    /// Catch-all for adapter-specific failures. Carries an
    /// agent-readable message; structured variants extend the enum.
    #[error("workspace store error: {0}")]
    Other(String),
}

/// Adapter trait — the seam between the engine and the persisted
/// workspace state. Implementations decide *where* the mount list
/// lives (two files under `.memstead/`, a SQLite database, a remote
/// service, an in-memory test fixture); the engine consumes the
/// produced [`Workspace`] uniformly.
pub trait WorkspaceStoreAdapter: Send + Sync {
    /// Load the workspace from `workspace_root`. Implementations
    /// resolve the adapter-specific files relative to this root
    /// (e.g. the file adapter reads
    /// `<workspace_root>/.memstead/workspace.toml` +
    /// `<workspace_root>/.memstead/state/mounts.json`).
    fn load(&self, workspace_root: &Path) -> Result<Workspace, StoreError>;

    /// Persist the engine-managed slice of state (today: the mount
    /// list). Operator-edited fields stay untouched — adapters that
    /// share one file with operator content must not overwrite it
    /// here. The two-layer file adapter writes only
    /// `state/mounts.json`.
    fn save_state(&self, workspace_root: &Path, workspace: &Workspace) -> Result<(), StoreError>;
}

/// Two-layer file adapter — the default. Reads
/// `.memstead/workspace.toml` (operator) +
/// `.memstead/state/mounts.json` (engine). Constructed without
/// arguments; everything is keyed off the workspace root passed
/// per-call.
#[derive(Debug, Default, Clone, Copy)]
pub struct FileWorkspaceStore;

impl FileWorkspaceStore {
    /// Construct the adapter. Stateless; safe to share across
    /// engines, callers, and tests.
    pub fn new() -> Self {
        Self
    }

    /// Path of the operator-edited file.
    pub fn workspace_toml_path(workspace_root: &Path) -> PathBuf {
        workspace_root
            .join(WORKSPACE_STORE_DIR)
            .join("workspace.toml")
    }

    /// Path of the engine-managed state file.
    pub fn mounts_json_path(workspace_root: &Path) -> PathBuf {
        workspace_root
            .join(WORKSPACE_STORE_DIR)
            .join("state")
            .join("mounts.json")
    }
}

const WORKSPACE_TOML_FORMAT: &str = "memstead-git-branch-2";
/// Pre-rename `workspace.toml` format. V1 carried the old unit-noun
/// policy tables. Recognised only to refuse with
/// [`StoreError::LegacyLayout`] — no dual-read.
const WORKSPACE_TOML_FORMAT_LEGACY: &str = "memstead-git-branch-1";
/// Current `mounts.json` format. V3 is the unit-noun cut: mount
/// records carry a `"mem"` field and mem-repo path values. Like V2 it
/// stores `gitdir` / `path` values relative to `workspace_root` when
/// they live inside the workspace, so checked-in state survives a
/// clone into a different home dir. Absolute paths are still accepted
/// on write (and round-trip untouched) when the mount target sits
/// outside `workspace_root` — e.g., an archive on a shared cache. The
/// reader resolves relative values against `workspace_root` at load
/// time.
const MOUNTS_JSON_FORMAT_V3: &str = "memstead-mounts-3";
/// Pre-rename `mounts.json` formats (V1: absolute paths; V2: relative
/// paths; both with the old unit-noun record field). Recognised only
/// to refuse with [`StoreError::LegacyLayout`] — there is no
/// dual-read; a one-shot migration rewrites state in place.
const MOUNTS_JSON_FORMAT_LEGACY: [&str; 2] = ["memstead-mounts-1", "memstead-mounts-2"];

/// Format-only probe for `mounts.json`, parsed before the full
/// document so format refusals (legacy layout, unknown version)
/// surface as typed errors rather than record-level parse failures.
#[derive(Deserialize)]
struct MountsFormatProbe {
    format: String,
}

/// Shared `workspace.toml` format gate: current passes, the
/// pre-rename V1 refuses as [`StoreError::LegacyLayout`], anything
/// else as [`StoreError::FormatMismatch`].
fn check_workspace_toml_format(format: &str, toml_path: &Path) -> Result<(), StoreError> {
    if format == WORKSPACE_TOML_FORMAT {
        return Ok(());
    }
    if format == WORKSPACE_TOML_FORMAT_LEGACY {
        return Err(StoreError::LegacyLayout {
            path: toml_path.to_path_buf(),
            found: format.to_string(),
        });
    }
    Err(StoreError::FormatMismatch {
        path: toml_path.to_path_buf(),
        expected: WORKSPACE_TOML_FORMAT.to_string(),
        found: format.to_string(),
    })
}

/// Resolve a path read from `mounts.json` against the workspace
/// root. Absolute paths are returned untouched (they sit outside
/// `workspace_root` by design — typically a shared archive cache);
/// relative paths are joined against `workspace_root` so the
/// in-memory `Mount` always carries an absolute path.
fn absolutize_mount_path(value: PathBuf, workspace_root: &Path) -> PathBuf {
    if value.is_absolute() {
        value
    } else {
        workspace_root.join(value)
    }
}

/// Normalise an absolute mount path for `mounts.json` serialisation.
/// When the path sits inside `workspace_root`, strip the prefix so
/// the on-disk form is portable. When it sits outside — an archive
/// in a global cache, a mem on a separate filesystem — keep the
/// absolute form; `strip_prefix` failure is the explicit fallback.
fn relativize_mount_path(value: &Path, workspace_root: &Path) -> PathBuf {
    match value.strip_prefix(workspace_root) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => value.to_path_buf(),
    }
}

/// True when `dir` is a workspace root: it carries
/// `.memstead/workspace.toml`. The shared recognition primitive for
/// every workspace walk-up (MCP boot, CLI setup, per-command walkers)
/// — keep them all on this helper so workspaces resolve uniformly.
pub fn is_workspace_root(dir: &Path) -> bool {
    FileWorkspaceStore::workspace_toml_path(dir).is_file()
}

impl WorkspaceStoreAdapter for FileWorkspaceStore {
    fn load(&self, workspace_root: &Path) -> Result<Workspace, StoreError> {
        let memstead_dir = workspace_root.join(WORKSPACE_STORE_DIR);
        if !memstead_dir.is_dir() {
            return Err(StoreError::NotInitialised {
                path: workspace_root.to_path_buf(),
            });
        }

        // workspace.toml is required (carries the adapter declaration).
        let toml_path = Self::workspace_toml_path(workspace_root);
        let toml_text = std::fs::read_to_string(&toml_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StoreError::NotInitialised {
                    path: workspace_root.to_path_buf(),
                }
            } else {
                StoreError::Io {
                    path: toml_path.clone(),
                    source: e,
                }
            }
        })?;
        let toml_doc: WorkspaceTomlDoc =
            toml::from_str(&toml_text).map_err(|e| StoreError::Parse {
                path: toml_path.clone(),
                message: e.to_string(),
            })?;
        check_workspace_toml_format(&toml_doc.format, &toml_path)?;

        // state/mounts.json is optional — a fresh workspace has the
        // adapter declaration but no mounts yet. Treat the missing
        // file as "zero mounts".
        let mounts_path = Self::mounts_json_path(workspace_root);
        let mounts: Vec<Mount> = match std::fs::read_to_string(&mounts_path) {
            Ok(text) => {
                // Probe the format field before the full parse: a
                // pre-rename file fails record deserialisation (old
                // unit-noun field name), and the typed LegacyLayout
                // refusal must win over that generic parse error.
                let probe: MountsFormatProbe =
                    serde_json::from_str(&text).map_err(|e| StoreError::Parse {
                        path: mounts_path.clone(),
                        message: e.to_string(),
                    })?;
                if MOUNTS_JSON_FORMAT_LEGACY.contains(&probe.format.as_str()) {
                    return Err(StoreError::LegacyLayout {
                        path: mounts_path,
                        found: probe.format,
                    });
                }
                if probe.format != MOUNTS_JSON_FORMAT_V3 {
                    return Err(StoreError::FormatMismatch {
                        path: mounts_path,
                        expected: MOUNTS_JSON_FORMAT_V3.to_string(),
                        found: probe.format,
                    });
                }
                let doc: MountsJsonDoc =
                    serde_json::from_str(&text).map_err(|e| StoreError::Parse {
                        path: mounts_path.clone(),
                        message: e.to_string(),
                    })?;
                doc.mounts
                    .into_iter()
                    .map(|w| w.into_mount(workspace_root))
                    .collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                return Err(StoreError::Io {
                    path: mounts_path,
                    source: e,
                });
            }
        };

        warn_if_legacy_schemas_dir(toml_doc.schemas_dir.as_deref());
        let settings = build_settings(
            toml_doc.mem_management,
            toml_doc.cross_mem_links,
            toml_doc.mcp,
            toml_doc.mutations,
            toml_doc.plugin,
        )?;
        Ok(Workspace { mounts, settings })
    }

    fn save_state(&self, workspace_root: &Path, workspace: &Workspace) -> Result<(), StoreError> {
        let mounts_path = Self::mounts_json_path(workspace_root);
        if let Some(parent) = mounts_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let doc = MountsJsonDoc {
            format: MOUNTS_JSON_FORMAT_V3.to_string(),
            mounts: workspace
                .mounts
                .iter()
                .map(|m| MountWire::from_mount(m, workspace_root))
                .collect(),
        };
        let text = serde_json::to_string_pretty(&doc).map_err(|e| StoreError::Parse {
            path: mounts_path.clone(),
            message: e.to_string(),
        })?;
        std::fs::write(&mounts_path, text).map_err(|e| StoreError::Io {
            path: mounts_path,
            source: e,
        })?;
        Ok(())
    }
}

/// On-disk shape of `workspace.toml`. Operator-edited; engine never
/// writes. V1 carries the adapter declaration, `[mem_management]`
/// rule lists, and `[cross_mem_links]` permission policy. Plugin
/// hooks land additively when consumers need them.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceTomlDoc {
    /// Schema version of the TOML file. Must equal
    /// `memstead-git-branch-1` for V1; mismatch surfaces
    /// [`StoreError::FormatMismatch`].
    format: String,
    /// Persistence-adapter declaration. Carries the adapter `name`
    /// (default: `"file-two-layer"`); future adapters add their own
    /// nested config blocks.
    #[serde(default)]
    persistence_adapter: PersistenceAdapterDecl,
    /// `[mem_management]` rule lists. Both arrays default to empty
    /// — an empty list means "no agent-driven mem create / delete
    /// allowed" (mirrors full). Operators add `[[mem_management.create]]`
    /// / `[[mem_management.delete]]` entries to opt in.
    #[serde(default)]
    mem_management: MemManagementWire,
    /// `[cross_mem_links]` raw shape — `<mem> = "*"` (wildcard)
    /// or `<mem> = ["target", ...]` (allowlist) per key. Parsed
    /// post-decode via `memstead_schema::workspace_config::CrossLinkValue::parse_toml`
    /// because the wildcard-or-list shape doesn't fit serde's
    /// untagged-enum pattern. Empty when the section is absent;
    /// interpreted as default-deny.
    #[serde(default)]
    cross_mem_links: toml::Table,
    /// **Retired key.** The folder-backend authored-schema location is
    /// fixed at `<workspace>/.memstead/schemas/`; this key is no longer
    /// honoured. Kept here only so an older workspace.toml that still
    /// carries it parses cleanly — `warn_if_legacy_schemas_dir` emits a
    /// one-line warning and the value is dropped (never threaded into
    /// `WorkspaceSettings`).
    #[serde(default)]
    schemas_dir: Option<std::path::PathBuf>,
    /// `[mcp]` section — MCP-binary tuning. Absent → defaults
    /// (`token_budget` falls back to the binary's compile-time
    /// default, `disabled_tools` is empty).
    #[serde(default)]
    mcp: McpSection,
    /// `[mutations]` section — engine-wide mutation policy. Absent →
    /// `require_notes = None` (interpreted as `false`).
    #[serde(default)]
    mutations: MutationsSection,
    /// `[plugin.*]` namespace — opaque pass-through map keyed by
    /// plugin identifier. Values are raw TOML tables; the engine
    /// never inspects them.
    #[serde(default)]
    plugin: std::collections::HashMap<String, toml::Table>,
}

/// Wire shape for the `[mem_management]` section. Both arrays
/// default to empty so the section may be omitted from
/// `workspace.toml` entirely.
#[derive(Debug, Default, Serialize, Deserialize)]
struct MemManagementWire {
    #[serde(default)]
    create: Vec<CreateRuleWire>,
    #[serde(default)]
    delete: Vec<DeleteRuleWire>,
}

/// Wire shape for one `[[mem_management.create]]` entry. Mirrors
/// [`crate::workspace::CreateRuleSetting`] with serde defaults so
/// `schemas` may be omitted (treated as the empty allowlist —
/// effectively a deny rule, surfaced for parity with full).
///
/// `default_cross_links` is decoded as a raw `toml::Value` and lifted
/// to `CrossLinkValue` post-decode via the same parser as the
/// top-level `[cross_mem_links]` section, sharing the wildcard-vs-
/// list-vs-mixed-rejection semantics.
#[derive(Debug, Serialize, Deserialize)]
struct CreateRuleWire {
    pattern: String,
    #[serde(default)]
    schemas: Vec<String>,
    #[serde(default)]
    default_cross_links: Option<toml::Value>,
}

/// Wire shape for one `[[mem_management.delete]]` entry. Mirrors
/// [`crate::workspace::DeleteRuleSetting`].
#[derive(Debug, Serialize, Deserialize)]
struct DeleteRuleWire {
    pattern: String,
}

/// Parse the operator-edited `.memstead/workspace.toml` at `workspace_root`
/// into a fresh `WorkspaceSettings`. Exposed so MCP- and CLI-driven
/// policy-mutation tools (the `workspace_config_edit::{grant,revoke}_*`
/// family) can refresh the engine's in-memory settings after writing
/// to disk — closing the stale-cache footgun where the next call
/// into the engine after a successful policy mutation still saw the
/// pre-mutation policy.
///
/// Reads only `workspace.toml` — the engine-managed `mounts.json` is
/// untouched. The function pays one file-read; the alternative
/// (threading projections through the policy-mutation functions
/// without an engine handle) was rejected for the coupling cost.
///
/// Errors mirror [`FileWorkspaceStore::load`]'s subset that touches
/// `workspace.toml` only: `NotInitialised` (no `.memstead/` dir),
/// `Io` / `Parse` (file read or TOML parse failure),
/// `FormatMismatch` (unsupported `format` field).
pub fn parse_workspace_settings(
    workspace_root: &Path,
) -> Result<crate::workspace::WorkspaceSettings, StoreError> {
    let memstead_dir = workspace_root.join(WORKSPACE_STORE_DIR);
    if !memstead_dir.is_dir() {
        return Err(StoreError::NotInitialised {
            path: workspace_root.to_path_buf(),
        });
    }
    let toml_path = FileWorkspaceStore::workspace_toml_path(workspace_root);
    let toml_text = std::fs::read_to_string(&toml_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            StoreError::NotInitialised {
                path: workspace_root.to_path_buf(),
            }
        } else {
            StoreError::Io {
                path: toml_path.clone(),
                source: e,
            }
        }
    })?;
    let toml_doc: WorkspaceTomlDoc = toml::from_str(&toml_text).map_err(|e| StoreError::Parse {
        path: toml_path.clone(),
        message: e.to_string(),
    })?;
    check_workspace_toml_format(&toml_doc.format, &toml_path)?;
    warn_if_legacy_schemas_dir(toml_doc.schemas_dir.as_deref());
    build_settings(
        toml_doc.mem_management,
        toml_doc.cross_mem_links,
        toml_doc.mcp,
        toml_doc.mutations,
        toml_doc.plugin,
    )
}

/// Build a `WorkspaceSettings` from the raw wire shapes. Folds in
/// the `[mem_management]` rules and the post-decoded
/// `[cross_mem_links]` map; surfaces a typed parse error if any
/// cross-link value violates the wildcard / list / non-empty
/// invariants.
fn build_settings(
    vm: MemManagementWire,
    cross_mem_links_raw: toml::Table,
    mcp: McpSection,
    mutations: MutationsSection,
    plugin: std::collections::HashMap<String, toml::Table>,
) -> Result<WorkspaceSettings, StoreError> {
    let mut create_rules = Vec::with_capacity(vm.create.len());
    for r in vm.create {
        let default_cross_links = match r.default_cross_links {
            None => None,
            Some(value) => {
                let location = format!(
                    "[[mem_management.create]] pattern={}.default_cross_links",
                    r.pattern
                );
                Some(parse_cross_link_value(&location, &value)?)
            }
        };
        create_rules.push(crate::workspace::CreateRuleSetting {
            pattern: r.pattern,
            schemas: r.schemas,
            default_cross_links,
        });
    }

    let mut cross_mem_links = std::collections::BTreeMap::new();
    for (mem, value) in &cross_mem_links_raw {
        let location = format!("[cross_mem_links].{mem}");
        let parsed = parse_cross_link_value(&location, value)?;
        cross_mem_links.insert(mem.clone(), parsed);
    }

    Ok(WorkspaceSettings {
        mem_create_rules: create_rules,
        mem_delete_rules: vm
            .delete
            .into_iter()
            .map(|r| crate::workspace::DeleteRuleSetting { pattern: r.pattern })
            .collect(),
        cross_mem_links,
        mcp,
        mutations,
        plugin,
    })
}

/// The folder-backend authored-schema location is fixed at
/// `<workspace>/.memstead/schemas/` — the `schemas_dir` workspace.toml
/// key is retired (no configurability without demonstrated need). A
/// workspace.toml that still carries the key gets a one-line warning
/// naming the fixed location; the key is otherwise ignored, never
/// honoured. Called from both `workspace.toml` parse entry points.
fn warn_if_legacy_schemas_dir(schemas_dir: Option<&std::path::Path>) {
    if let Some(dir) = schemas_dir {
        tracing::warn!(
            "`schemas_dir` (= {:?}) in workspace.toml is retired and ignored — \
             authored schemas are read from the fixed `<workspace>/.memstead/schemas/`. \
             Remove the key to silence this warning.",
            dir
        );
    }
}

/// Parse one cross-link value via `memstead_schema::workspace_config::CrossLinkValue::parse_toml`,
/// lifting the schema-crate's `ConfigError` into a `StoreError::Parse` with
/// the operator-facing TOML location prefix.
fn parse_cross_link_value(
    location: &str,
    value: &toml::Value,
) -> Result<memstead_schema::workspace_config::CrossLinkValue, StoreError> {
    memstead_schema::workspace_config::CrossLinkValue::parse_toml(location, value).map_err(|e| {
        StoreError::Parse {
            path: std::path::PathBuf::from("workspace.toml"),
            message: e.to_string(),
        }
    })
}

/// Persistence-adapter section of `workspace.toml`. Future adapters
/// nest their config under this section.
#[derive(Debug, Serialize, Deserialize)]
struct PersistenceAdapterDecl {
    name: String,
}

impl Default for PersistenceAdapterDecl {
    fn default() -> Self {
        Self {
            name: "file-two-layer".to_string(),
        }
    }
}

/// On-disk shape of `state/mounts.json`. Engine-managed; operator
/// does not edit during normal operation (lifecycle tools rewrite it
/// after every mount-state mutation).
#[derive(Debug, Serialize, Deserialize)]
struct MountsJsonDoc {
    format: String,
    mounts: Vec<MountWire>,
}

/// Wire shape for one mount. Mirrors [`Mount`] with serializable
/// fields. Schema pin serialises as a plain string (`"default"` or
/// `"default@1.0.0"`); storage uses an internally-tagged enum
/// (`type: "folder" | "git-branch" | "archive"`).
#[derive(Debug, Serialize, Deserialize)]
struct MountWire {
    mem: String,
    /// Optional schema-pin *expectation assertion* (`<name>@<version>`).
    /// The authoritative pin is the mem's own `MemConfig.schema`;
    /// this is a workspace-local cross-check. `default` on read keeps
    /// older `mounts.json` files (which always carried the key) loading
    /// as `Some`; skip-on-`None` omits the key for assertion-less mounts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schema: Option<String>,
    /// In-flight migration target (`<name>@<version>`), absent for
    /// settled mems. `default` on read keeps pre-dual-pin
    /// `mounts.json` files loading unchanged; skip-on-`None` keeps
    /// settled mems' entries byte-identical to before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    migration_target: Option<String>,
    storage: MountStorageWire,
    capability: CapabilityWire,
    lifecycle: LifecycleWire,
    cross_linkable: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum MountStorageWire {
    Folder {
        path: PathBuf,
    },
    GitBranch {
        gitdir: PathBuf,
        branch: String,
    },
    Archive {
        path: PathBuf,
    },
    /// In-memory backend. Carries no fields — it serialises as the
    /// bare tag `{ "type": "in-memory" }`. Unambiguous against the
    /// other three variants (each of which carries a `path` or
    /// `gitdir`/`branch`), so a round-trip never confuses it for one
    /// of them. Present for wire completeness; ephemeral session
    /// mems are normally constructed via `Engine::from_mounts`
    /// rather than persisted to `mounts.json`.
    InMemory,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CapabilityWire {
    ReadOnly,
    Write,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum LifecycleWire {
    Eager,
    Lazy,
}

impl MountWire {
    fn from_mount(m: &Mount, workspace_root: &Path) -> Self {
        Self {
            mem: m.mem.clone(),
            schema: m.schema.as_ref().map(|s| s.to_string()),
            migration_target: m.migration_target.as_ref().map(|t| t.to_string()),
            storage: match &m.storage {
                MountStorage::Folder { path } => MountStorageWire::Folder {
                    path: relativize_mount_path(path, workspace_root),
                },
                MountStorage::GitBranch { gitdir, branch } => MountStorageWire::GitBranch {
                    gitdir: relativize_mount_path(gitdir, workspace_root),
                    branch: branch.clone(),
                },
                MountStorage::Archive { path } => MountStorageWire::Archive {
                    path: relativize_mount_path(path, workspace_root),
                },
                MountStorage::InMemory => MountStorageWire::InMemory,
            },
            capability: match m.capability {
                MountCapability::ReadOnly => CapabilityWire::ReadOnly,
                MountCapability::Write => CapabilityWire::Write,
            },
            lifecycle: match m.lifecycle {
                MountLifecycle::Eager => LifecycleWire::Eager,
                MountLifecycle::Lazy => LifecycleWire::Lazy,
            },
            cross_linkable: m.cross_linkable,
        }
    }

    fn into_mount(self, workspace_root: &Path) -> Mount {
        Mount {
            mem: self.mem,
            schema: self.schema.map(|s| {
                s.parse()
                    .expect("schema pin on disk must be `<name>@<version>`")
            }),
            migration_target: self.migration_target.map(|t| {
                t.parse()
                    .expect("migration_target on disk must be `<name>@<version>`")
            }),
            storage: match self.storage {
                MountStorageWire::Folder { path } => MountStorage::Folder {
                    path: absolutize_mount_path(path, workspace_root),
                },
                MountStorageWire::GitBranch { gitdir, branch } => MountStorage::GitBranch {
                    gitdir: absolutize_mount_path(gitdir, workspace_root),
                    branch,
                },
                MountStorageWire::Archive { path } => MountStorage::Archive {
                    path: absolutize_mount_path(path, workspace_root),
                },
                MountStorageWire::InMemory => MountStorage::InMemory,
            },
            capability: match self.capability {
                CapabilityWire::ReadOnly => MountCapability::ReadOnly,
                CapabilityWire::Write => MountCapability::Write,
            },
            lifecycle: match self.lifecycle {
                LifecycleWire::Eager => MountLifecycle::Eager,
                LifecycleWire::Lazy => MountLifecycle::Lazy,
            },
            cross_linkable: self.cross_linkable,
        }
    }
}

/// Errors surfaced by [`instantiate_lean_backend`].
#[derive(Debug, thiserror::Error)]
pub enum InstantiateError {
    /// Mount declares a `MountStorage::GitBranch` storage variant
    /// but the lean flavour cannot construct a git-branch backend
    /// (the implementation lives in `memstead-git-branch` behind the
    /// `mem-repo` Cargo feature). Full consumers expose a
    /// feature-gated `instantiate_full_backend` that handles all
    /// three variants.
    #[error(
        "mem {mem}: git-branch backend requires the `mem-repo` feature; \
         use `instantiate_full_backend` from memstead-git-branch, or rebuild with --features mem-repo"
    )]
    GitBranchRequiresMemRepoFeature { mem: String },
}

/// Materialise a [`MemBackend`] for `mount` using the lean-flavour
/// backends (folder + archive). Returns an error for the git-branch
/// variant — full consumers handle that with a feature-gated
/// counterpart in `memstead-git-branch`.
///
/// Lives in `memstead-base` because both folder and archive backends are
/// always-on; the function shape (one mount in, one boxed backend
/// out) stays uniform for both flavours so the engine's
/// `from_mounts` glue is identical between lean and full.
pub fn instantiate_lean_backend(mount: &Mount) -> Result<Box<dyn MemBackend>, InstantiateError> {
    match &mount.storage {
        MountStorage::Folder { path } => Ok(Box::new(FilesystemMemWriter::new(path.clone()))),
        MountStorage::Archive { path } => Ok(Box::new(ArchiveBackend::new(path.clone()))),
        MountStorage::InMemory => Ok(Box::new(InMemoryBackend::new())),
        MountStorage::GitBranch { .. } => Err(InstantiateError::GitBranchRequiresMemRepoFeature {
            mem: mount.mem.clone(),
        }),
    }
}

/// On-disk layout the workspace root carries today.
///
/// Drives [`Engine::from_workspace_root`](crate::Engine::from_workspace_root):
/// a workspace either carries the two-layer file adapter shape
/// (`Layout::New`) or it does not (`Layout::Empty`). Pre-rebuild
/// layouts are no longer recognised — operators run
/// `memstead mem-repo init` to bootstrap a fresh workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// No `.memstead/workspace.toml` present — operator should run
    /// `memstead mem-repo init` to bootstrap.
    Empty,
    /// `.memstead/workspace.toml` present — workspace runs on the
    /// two-layer file adapter.
    New,
}

/// Detect the on-disk layout at `workspace_root`. The returned
/// [`Layout`] discriminator is total: every workspace falls into
/// exactly one variant.
pub fn detect_layout(workspace_root: &Path) -> Layout {
    if is_workspace_root(workspace_root) {
        Layout::New
    } else {
        Layout::Empty
    }
}

/// Synthesize a one-mount [`Workspace`] from a bare *standalone* folder
/// mem — a directory carrying `.memstead/config.json` but **no**
/// `.memstead/workspace.toml`. The mem root *is* the workspace root
/// (the collapsed single-mem form), so the lone mount is a folder
/// backend pointed at `workspace_root` itself.
///
/// Returns `None` when the directory is not a standalone mem — no
/// readable, schema-pinned `config.json` — so boot callers fall through
/// to [`crate::BootError::NotInitialised`] exactly as before. This is what
/// collapses the old separate standalone-mem boot path into the unified
/// roster+detail experience: a lone mem opens as a workspace with one
/// mount, no `workspace.toml` required.
///
/// The synthesized workspace carries default (empty) settings — a
/// standalone mem has no `[mem_management]` / `[cross_mem_links]`
/// policy — and the mount is writable, not cross-linkable (there is no
/// sibling to link to). The schema pin and name come from the mem's own
/// `config.json`; an engine-written config omits `name`, so the directory
/// basename is the fallback identity.
pub fn standalone_workspace(workspace_root: &Path) -> Option<Workspace> {
    let config = memstead_schema::config::load_and_validate(workspace_root).ok()?;
    let schema = config.schema.clone()?;
    let name = config.name.clone().unwrap_or_else(|| {
        workspace_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "mem".to_string())
    });
    let mount = Mount {
        mem: name,
        schema: Some(schema),
        storage: MountStorage::Folder {
            path: workspace_root.to_path_buf(),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: false,
        migration_target: None,
    };
    Some(Workspace {
        mounts: vec![mount],
        settings: WorkspaceSettings::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use memstead_schema::SchemaRef;
    use std::io::Write as _;
    use tempfile::TempDir;

    fn pin(s: &str) -> SchemaRef {
        s.parse().unwrap()
    }

    fn folder_mount(mem: &str, path: PathBuf) -> Mount {
        Mount {
            mem: mem.to_string(),
            schema: Some(pin("default@1.0.0")),
            storage: MountStorage::Folder { path },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }
    }

    fn write_workspace_toml(workspace_root: &Path, body: &str) {
        let path = FileWorkspaceStore::workspace_toml_path(workspace_root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn load_returns_not_initialised_when_memstead_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let store = FileWorkspaceStore::new();
        let err = store.load(tmp.path()).unwrap_err();
        assert!(matches!(err, StoreError::NotInitialised { .. }));
    }

    /// The [`parse_workspace_settings`] helper reads only
    /// `.memstead/workspace.toml` and returns a fresh `WorkspaceSettings`.
    /// MCP-driven policy mutations call this after writing to disk
    /// to refresh the engine's in-memory cache without re-loading
    /// the engine-managed `mounts.json`.
    #[test]
    fn parse_workspace_settings_reflects_cross_mem_links_edit() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[cross_mem_links]
team-a = ["team-b"]
"#,
        );
        let settings = super::parse_workspace_settings(tmp.path()).unwrap();
        assert!(
            settings.cross_mem_links.contains_key("team-a"),
            "initial parse must surface the team-a grant; got {:?}",
            settings.cross_mem_links
        );

        // Mutate the file (simulating `workspace_config_edit::revoke_cross_link`).
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[cross_mem_links]
"#,
        );
        let refreshed = super::parse_workspace_settings(tmp.path()).unwrap();
        assert!(
            refreshed.cross_mem_links.is_empty(),
            "refreshed parse must drop the team-a grant; got {:?}",
            refreshed.cross_mem_links
        );
    }

    /// `parse_workspace_settings` surfaces
    /// `[[mem_management.create]]` / `[[mem_management.delete]]`
    /// rules so the engine's allowlist gate sees the post-mutation
    /// state immediately.
    #[test]
    fn parse_workspace_settings_reflects_allowlist_edit() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let initial = super::parse_workspace_settings(tmp.path()).unwrap();
        assert!(initial.mem_create_rules.is_empty());

        // Mutate (simulating `workspace_config_edit::add_create_rule`).
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[[mem_management.create]]
pattern = "test-*"
schemas = ["default@1.0.0"]
"#,
        );
        let refreshed = super::parse_workspace_settings(tmp.path()).unwrap();
        assert_eq!(refreshed.mem_create_rules.len(), 1);
        assert_eq!(refreshed.mem_create_rules[0].pattern, "test-*");
    }

    #[test]
    fn load_returns_not_initialised_when_workspace_toml_missing() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
        let store = FileWorkspaceStore::new();
        let err = store.load(tmp.path()).unwrap_err();
        assert!(matches!(err, StoreError::NotInitialised { .. }));
    }

    #[test]
    fn load_with_no_mounts_yields_empty_mount_list() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let store = FileWorkspaceStore::new();
        let workspace = store.load(tmp.path()).unwrap();
        assert!(workspace.mounts.is_empty());
    }

    #[test]
    fn load_with_no_mem_management_yields_empty_settings() {
        // Workspace.toml without a `[mem_management]` section
        // produces the default empty settings — mirrors full's
        // behaviour where missing rules mean "no agent-driven
        // mem create / delete allowed".
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let store = FileWorkspaceStore::new();
        let workspace = store.load(tmp.path()).unwrap();
        assert!(workspace.settings.mem_create_rules.is_empty());
        assert!(workspace.settings.mem_delete_rules.is_empty());
        assert!(workspace.settings.cross_mem_links.is_empty());
    }

    #[test]
    fn load_picks_up_cross_mem_links_wildcard_and_list() {
        // [cross_mem_links] is parsed via CrossLinkValue::parse_toml
        // to handle the wildcard ("*") vs allowlist ([...]) shape that
        // serde untagged-enum decode can't express. Both shapes round
        // through to WorkspaceSettings.cross_mem_links.
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[cross_mem_links]
specs = "*"
engine = ["specs", "macos"]
locked = []
"#,
        );
        let store = FileWorkspaceStore::new();
        let workspace = store.load(tmp.path()).unwrap();
        let cvl = &workspace.settings.cross_mem_links;
        assert_eq!(cvl.len(), 3);
        assert_eq!(cvl.get("specs"), Some(&CrossLinkValue::Wildcard));
        assert_eq!(
            cvl.get("engine"),
            Some(&CrossLinkValue::List(vec![
                "specs".to_string(),
                "macos".to_string()
            ]))
        );
        assert_eq!(cvl.get("locked"), Some(&CrossLinkValue::List(vec![])));
    }

    #[test]
    fn load_rejects_cross_mem_links_mixed_wildcard_and_names() {
        // The shared parser rejects `["*", "specs"]` — wildcard must
        // be the sole entry. The schema-crate's typed error lifts via
        // StoreError::Parse so the operator-facing error names the
        // exact key.
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[cross_mem_links]
specs = ["*", "engine"]
"#,
        );
        let store = FileWorkspaceStore::new();
        let err = store.load(tmp.path()).unwrap_err();
        match err {
            StoreError::Parse { message, .. } => {
                assert!(message.contains("[cross_mem_links].specs"));
                assert!(message.contains("wildcard"));
            }
            other => panic!("expected StoreError::Parse, got {other:?}"),
        }
    }

    #[test]
    fn load_picks_up_default_cross_links_on_create_rule() {
        // CreateRule.default_cross_links uses the same CrossLinkValue
        // parser. A rule with `default_cross_links = "*"` lifts to
        // CreateRuleSetting.default_cross_links = Some(Wildcard).
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[[mem_management.create]]
pattern = "exec-*"
schemas = ["default"]
default_cross_links = "*"
"#,
        );
        let store = FileWorkspaceStore::new();
        let workspace = store.load(tmp.path()).unwrap();
        let rule = &workspace.settings.mem_create_rules[0];
        assert_eq!(rule.pattern, "exec-*");
        assert_eq!(rule.default_cross_links, Some(CrossLinkValue::Wildcard));
    }

    #[test]
    fn load_picks_up_mem_management_create_and_delete_rules() {
        // Operator-edited `[mem_management]` section flows through
        // FileWorkspaceStore::load into Workspace.settings; the
        // engine layer then propagates via Engine::set_settings.
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[[mem_management.create]]
pattern = "exec-*"
schemas = ["default@1.0.0", "*"]

[[mem_management.create]]
pattern = "scratch-*"
schemas = ["default"]

[[mem_management.delete]]
pattern = "exec-*"
"#,
        );
        let store = FileWorkspaceStore::new();
        let workspace = store.load(tmp.path()).unwrap();
        assert_eq!(workspace.settings.mem_create_rules.len(), 2);
        assert_eq!(workspace.settings.mem_create_rules[0].pattern, "exec-*");
        assert_eq!(
            workspace.settings.mem_create_rules[0].schemas,
            vec!["default@1.0.0".to_string(), "*".to_string()]
        );
        assert_eq!(workspace.settings.mem_create_rules[1].pattern, "scratch-*");
        assert_eq!(workspace.settings.mem_delete_rules.len(), 1);
        assert_eq!(workspace.settings.mem_delete_rules[0].pattern, "exec-*");
    }

    /// Dual-pin state survives the store round-trip: a mount carrying
    /// `migration_target` writes it to `mounts.json` and reads it
    /// back; settled mounts' entries stay byte-compatible (the key is
    /// skipped when `None`).
    #[test]
    fn save_state_round_trips_migration_target() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            "\nformat = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        );
        let store = FileWorkspaceStore::new();
        let mut migrating = folder_mount("specs", PathBuf::from("/work/mem"));
        migrating.migration_target = Some(pin("mig-b@0.1.0"));
        let settled = folder_mount("other", PathBuf::from("/work/other"));
        let original = Workspace {
            mounts: vec![migrating, settled],
            settings: WorkspaceSettings::default(),
        };
        store.save_state(tmp.path(), &original).unwrap();
        let raw =
            std::fs::read_to_string(FileWorkspaceStore::mounts_json_path(tmp.path())).unwrap();
        assert!(
            raw.contains("mig-b@0.1.0"),
            "migration_target must persist: {raw}"
        );
        assert_eq!(
            raw.matches("migration_target").count(),
            1,
            "settled mounts must omit the key entirely: {raw}"
        );
        let loaded = store.load(tmp.path()).unwrap();
        assert_eq!(loaded.mounts[0].migration_target, Some(pin("mig-b@0.1.0")));
        assert_eq!(loaded.mounts[1].migration_target, None);
    }

    #[test]
    fn save_state_then_load_round_trips_mount_list() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let store = FileWorkspaceStore::new();
        let original = Workspace {
            mounts: vec![
                folder_mount("specs", PathBuf::from("/work/mem")),
                Mount {
                    mem: "engine".to_string(),
                    schema: Some(pin("default@1.0.0")),
                    storage: MountStorage::GitBranch {
                        gitdir: PathBuf::from("/work/mem-repo/.git"),
                        branch: "engine".to_string(),
                    },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: true,
                    migration_target: None,
                },
                Mount {
                    mem: "external".to_string(),
                    schema: Some(pin("default@1.0.0")),
                    storage: MountStorage::Archive {
                        path: PathBuf::from("/deps/external.mem"),
                    },
                    capability: MountCapability::ReadOnly,
                    lifecycle: MountLifecycle::Lazy,
                    cross_linkable: false,
                    migration_target: None,
                },
            ],
            settings: WorkspaceSettings::default(),
        };
        store.save_state(tmp.path(), &original).unwrap();

        // The mounts.json file lives where we expect.
        assert!(FileWorkspaceStore::mounts_json_path(tmp.path()).is_file());

        let reloaded = store.load(tmp.path()).unwrap();
        assert_eq!(reloaded.mounts.len(), original.mounts.len());
        for (a, b) in reloaded.mounts.iter().zip(original.mounts.iter()) {
            assert_eq!(a.mem, b.mem);
            assert_eq!(a.schema, b.schema);
            assert_eq!(a.capability, b.capability);
            assert_eq!(a.lifecycle, b.lifecycle);
            assert_eq!(a.cross_linkable, b.cross_linkable);
            assert_eq!(a.storage, b.storage);
        }
    }

    #[test]
    fn save_state_round_trips_unset_schema_assertion() {
        // `Mount.schema = None` (no expectation assertion) must survive a
        // mounts.json round-trip and omit the `schema` key on the wire —
        // the authoritative pin lives in the mem's backend config, not
        // in the mount record.
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let store = FileWorkspaceStore::new();
        let original = Workspace {
            mounts: vec![Mount {
                mem: "foreign".to_string(),
                schema: None,
                storage: MountStorage::Folder {
                    path: tmp.path().join("foreign"),
                },
                capability: MountCapability::ReadOnly,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: false,
                migration_target: None,
            }],
            settings: WorkspaceSettings::default(),
        };
        store.save_state(tmp.path(), &original).unwrap();

        // The wire form omits the `schema` key entirely (skip-on-None).
        let raw =
            std::fs::read_to_string(FileWorkspaceStore::mounts_json_path(tmp.path())).unwrap();
        assert!(
            !raw.contains("\"schema\""),
            "unset schema assertion must omit the key on the wire; got:\n{raw}"
        );

        // And it reloads as `None`.
        let reloaded = store.load(tmp.path()).unwrap();
        assert_eq!(reloaded.mounts.len(), 1);
        assert_eq!(reloaded.mounts[0].schema, None);
    }

    #[test]
    fn save_state_does_not_touch_workspace_toml() {
        let tmp = TempDir::new().unwrap();
        let original_body = r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#;
        write_workspace_toml(tmp.path(), original_body);
        let store = FileWorkspaceStore::new();
        let workspace = Workspace::default();
        store.save_state(tmp.path(), &workspace).unwrap();
        // Operator's TOML untouched.
        let toml_after =
            std::fs::read_to_string(FileWorkspaceStore::workspace_toml_path(tmp.path())).unwrap();
        assert_eq!(toml_after, original_body);
    }

    #[test]
    fn save_state_writes_paths_relative_to_workspace_root() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let store = FileWorkspaceStore::new();
        let workspace = Workspace {
            mounts: vec![
                Mount {
                    mem: "engine".to_string(),
                    schema: Some(pin("default@1.0.0")),
                    storage: MountStorage::GitBranch {
                        gitdir: tmp.path().join("mem-repo").join(".git"),
                        branch: "engine".to_string(),
                    },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: true,
                    migration_target: None,
                },
                Mount {
                    mem: "external".to_string(),
                    schema: Some(pin("default@1.0.0")),
                    storage: MountStorage::Archive {
                        path: PathBuf::from("/global/cache/external.mem"),
                    },
                    capability: MountCapability::ReadOnly,
                    lifecycle: MountLifecycle::Lazy,
                    cross_linkable: false,
                    migration_target: None,
                },
            ],
            settings: WorkspaceSettings::default(),
        };
        store.save_state(tmp.path(), &workspace).unwrap();

        let on_disk =
            std::fs::read_to_string(FileWorkspaceStore::mounts_json_path(tmp.path())).unwrap();
        // Format bumped to V2.
        assert!(on_disk.contains("\"memstead-mounts-3\""));
        // In-workspace path stored relative — no absolute prefix bake-in.
        assert!(
            on_disk.contains("\"mem-repo/.git\""),
            "expected relative gitdir, got: {on_disk}"
        );
        assert!(
            !on_disk.contains(tmp.path().to_str().unwrap()),
            "in-workspace path should not include the absolute tmp prefix: {on_disk}"
        );
        // Out-of-workspace path kept absolute (fallback for shared caches / external archives).
        assert!(on_disk.contains("\"/global/cache/external.mem\""));

        // Re-load reconstructs absolute paths.
        let reloaded = store.load(tmp.path()).unwrap();
        match &reloaded.mounts[0].storage {
            MountStorage::GitBranch { gitdir, .. } => {
                assert_eq!(gitdir, &tmp.path().join("mem-repo").join(".git"));
            }
            other => panic!("expected GitBranch storage, got {other:?}"),
        }
        match &reloaded.mounts[1].storage {
            MountStorage::Archive { path } => {
                assert_eq!(path, &PathBuf::from("/global/cache/external.mem"));
            }
            other => panic!("expected Archive storage, got {other:?}"),
        }
    }

    #[test]
    fn load_absolute_inside_root_path_then_save_rewrites_relative() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        // Hand-write a mounts.json carrying an absolute gitdir that
        // lives inside this workspace_root — simulates the "committed
        // by another operator's home dir" failure mode that motivated
        // relative serialisation.
        let mounts_path = FileWorkspaceStore::mounts_json_path(tmp.path());
        std::fs::create_dir_all(mounts_path.parent().unwrap()).unwrap();
        let abs_gitdir = tmp.path().join("mem-repo").join(".git");
        let mounts_body = format!(
            r#"{{
  "format": "memstead-mounts-3",
  "mounts": [
    {{
      "mem": "engine",
      "schema": "default@1.0.0",
      "storage": {{
        "type": "git-branch",
        "gitdir": "{}",
        "branch": "engine"
      }},
      "capability": "write",
      "lifecycle": "eager",
      "cross_linkable": true
    }}
  ]
}}"#,
            abs_gitdir.to_str().unwrap()
        );
        std::fs::write(&mounts_path, &mounts_body).unwrap();

        let store = FileWorkspaceStore::new();
        // The reader accepts the file; the absolute path round-trips
        // untouched (it's already absolute).
        let workspace = store.load(tmp.path()).unwrap();
        match &workspace.mounts[0].storage {
            MountStorage::GitBranch { gitdir, .. } => assert_eq!(gitdir, &abs_gitdir),
            other => panic!("expected GitBranch storage, got {other:?}"),
        }

        // Saving the same workspace rewrites the file with a relative
        // path — self-healing, no explicit command needed.
        store.save_state(tmp.path(), &workspace).unwrap();
        let on_disk = std::fs::read_to_string(&mounts_path).unwrap();
        assert!(on_disk.contains("\"memstead-mounts-3\""));
        assert!(on_disk.contains("\"mem-repo/.git\""));
        assert!(!on_disk.contains(tmp.path().to_str().unwrap()));
    }

    /// Item 03 round-trip: writing a `MountStorage::GitBranch` mount
    /// whose `branch` already carries the canonical `refs/heads/<leaf>`
    /// form serialises that exact string into `mounts.json` (no
    /// rewrite, no truncation), and reading the file back produces a
    /// `Mount` whose in-memory `branch` equals the input verbatim. The
    /// write path is the one source of truth for the on-disk shape — a
    /// regression that re-introduces short-form writes would surface
    /// here as a mismatch against `refs/heads/demo/engine`.
    #[test]
    fn save_state_preserves_refs_heads_branch_form() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let store = FileWorkspaceStore::new();
        let original = Workspace {
            mounts: vec![Mount {
                mem: "engine".to_string(),
                schema: Some(pin("default@1.0.0")),
                storage: MountStorage::GitBranch {
                    gitdir: tmp.path().join("mem-repo").join(".git"),
                    branch: "refs/heads/demo/engine".to_string(),
                },
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            }],
            settings: WorkspaceSettings::default(),
        };
        store.save_state(tmp.path(), &original).unwrap();

        let on_disk =
            std::fs::read_to_string(FileWorkspaceStore::mounts_json_path(tmp.path())).unwrap();
        assert!(
            on_disk.contains("\"branch\": \"refs/heads/demo/engine\""),
            "expected fully-qualified ref on disk, got: {on_disk}"
        );

        let reloaded = store.load(tmp.path()).unwrap();
        match &reloaded.mounts[0].storage {
            MountStorage::GitBranch { branch, .. } => {
                assert_eq!(branch, "refs/heads/demo/engine");
            }
            other => panic!("expected GitBranch storage, got {other:?}"),
        }
    }

    /// Item 03 reader tolerance: a hand-written legacy `mounts.json`
    /// whose `branch` field carries the short-form leaf (no
    /// `refs/heads/` prefix) loads without error, and the in-memory
    /// `Mount` carries the input string intact. The reader does not
    /// silently normalise — backend factories are responsible for
    /// fully-qualifying short forms at instantiation time. This pin
    /// guards against an over-eager normaliser landing on the read
    /// path and masking out the legacy shape that older committed
    /// `mounts.json` files used to carry.
    #[test]
    fn load_preserves_short_form_branch_without_rewrite() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let mounts_path = FileWorkspaceStore::mounts_json_path(tmp.path());
        std::fs::create_dir_all(mounts_path.parent().unwrap()).unwrap();
        std::fs::write(
            &mounts_path,
            r#"{
  "format": "memstead-mounts-3",
  "mounts": [
    {
      "mem": "engine",
      "schema": "default@1.0.0",
      "storage": {
        "type": "git-branch",
        "gitdir": "mem-repo/.git",
        "branch": "demo/engine"
      },
      "capability": "write",
      "lifecycle": "eager",
      "cross_linkable": true
    }
  ]
}"#,
        )
        .unwrap();

        let store = FileWorkspaceStore::new();
        let workspace = store.load(tmp.path()).unwrap();
        match &workspace.mounts[0].storage {
            MountStorage::GitBranch { branch, .. } => {
                assert_eq!(
                    branch, "demo/engine",
                    "reader must not silently rewrite short-form branch"
                );
            }
            other => panic!("expected GitBranch storage, got {other:?}"),
        }
    }

    #[test]
    fn load_rejects_format_version_mismatch_on_toml() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-99"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let store = FileWorkspaceStore::new();
        let err = store.load(tmp.path()).unwrap_err();
        match err {
            StoreError::FormatMismatch {
                expected, found, ..
            } => {
                assert_eq!(expected, "memstead-git-branch-2");
                assert_eq!(found, "memstead-git-branch-99");
            }
            other => panic!("expected FormatMismatch, got {other:?}"),
        }
    }

    #[test]
    fn load_rejects_format_version_mismatch_on_mounts_json() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let mounts_path = FileWorkspaceStore::mounts_json_path(tmp.path());
        std::fs::create_dir_all(mounts_path.parent().unwrap()).unwrap();
        std::fs::write(
            &mounts_path,
            r#"{ "format": "memstead-mounts-99", "mounts": [] }"#,
        )
        .unwrap();
        let store = FileWorkspaceStore::new();
        let err = store.load(tmp.path()).unwrap_err();
        assert!(matches!(err, StoreError::FormatMismatch { .. }));
    }

    /// A pre-rename workspace.toml (format V1) refuses with the typed
    /// LegacyLayout error — it must not boot empty or half-parsed.
    #[test]
    fn load_refuses_pre_rename_toml_as_legacy_layout() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-1"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let store = FileWorkspaceStore::new();
        let err = store.load(tmp.path()).unwrap_err();
        match err {
            StoreError::LegacyLayout { found, .. } => {
                assert_eq!(found, "memstead-git-branch-1");
            }
            other => panic!("expected LegacyLayout, got {other:?}"),
        }
    }

    /// Pre-rename mounts.json formats refuse with LegacyLayout even
    /// though their records no longer deserialise (old unit-noun
    /// field name) — the format probe must win over the record-level
    /// parse error so the agent sees the migration hint, not a serde
    /// message. The fixture's record deliberately lacks the `mem`
    /// field to prove the full parse is never reached.
    #[test]
    fn load_refuses_pre_rename_mounts_json_as_legacy_layout() {
        for legacy in ["memstead-mounts-1", "memstead-mounts-2"] {
            let tmp = TempDir::new().unwrap();
            write_workspace_toml(
                tmp.path(),
                r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
            );
            let mounts_path = FileWorkspaceStore::mounts_json_path(tmp.path());
            std::fs::create_dir_all(mounts_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mounts_path,
                format!(
                    r#"{{ "format": "{legacy}", "mounts": [{{ "unit": "notes", "storage": {{ "type": "folder", "path": "notes" }}, "capability": "write", "lifecycle": "eager", "cross_linkable": true }}] }}"#
                ),
            )
            .unwrap();
            let store = FileWorkspaceStore::new();
            let err = store.load(tmp.path()).unwrap_err();
            match err {
                StoreError::LegacyLayout { found, .. } => assert_eq!(found, legacy),
                other => panic!("expected LegacyLayout for {legacy}, got {other:?}"),
            }
        }
    }

    #[test]
    fn load_rejects_invalid_toml() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(tmp.path(), "this is not = valid = toml");
        let store = FileWorkspaceStore::new();
        let err = store.load(tmp.path()).unwrap_err();
        assert!(matches!(err, StoreError::Parse { .. }));
    }

    #[test]
    fn load_rejects_unknown_top_level_key() {
        // The workspace config's contract (and its shipped example's
        // claim) is that typos never pass silently — at the top level,
        // not just inside [mcp]/[mutations].
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            "format = \"memstead-git-branch-2\"\nnonexistent_key = true\n",
        );
        let store = FileWorkspaceStore::new();
        let err = store.load(tmp.path()).unwrap_err();
        match err {
            StoreError::Parse { message, .. } => {
                assert!(
                    message.contains("nonexistent_key"),
                    "refusal must name the unknown key: {message}"
                );
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn instantiate_lean_backend_handles_folder_archive_and_in_memory() {
        let tmp = TempDir::new().unwrap();
        let folder = folder_mount("local", tmp.path().to_path_buf());
        let archive_path = tmp.path().join("ext.mem");
        // Make a minimal valid zip so the archive backend can open it.
        let f = std::fs::File::create(&archive_path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        w.start_file("a.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(b"# a").unwrap();
        w.finish().unwrap();
        let archive = Mount {
            mem: "external".to_string(),
            schema: Some(pin("default@1.0.0")),
            storage: MountStorage::Archive { path: archive_path },
            capability: MountCapability::ReadOnly,
            lifecycle: MountLifecycle::Lazy,
            cross_linkable: false,
            migration_target: None,
        };
        let in_memory = Mount {
            mem: "session".to_string(),
            schema: Some(pin("default@1.0.0")),
            storage: MountStorage::InMemory,
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };

        let _: Box<dyn MemBackend> = instantiate_lean_backend(&folder).unwrap();
        let _: Box<dyn MemBackend> = instantiate_lean_backend(&archive).unwrap();
        // The in-memory variant is a lean backend — no feature gate,
        // no path, materialises directly.
        let _: Box<dyn MemBackend> = instantiate_lean_backend(&in_memory).unwrap();
    }

    /// AC3 (plan 01): the in-memory storage variant round-trips through
    /// `mounts.json` and its wire shape is unambiguous — it serialises
    /// as the bare `{"type":"in-memory"}` tag and never parses as one
    /// of the path-carrying variants, nor they as it.
    #[test]
    fn save_state_round_trips_in_memory_variant_unambiguously() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let store = FileWorkspaceStore::new();
        let original = Workspace {
            mounts: vec![
                folder_mount("local", PathBuf::from("/work/mem")),
                Mount {
                    mem: "session".to_string(),
                    schema: Some(pin("default@1.0.0")),
                    storage: MountStorage::InMemory,
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: true,
                    migration_target: None,
                },
            ],
            settings: WorkspaceSettings::default(),
        };
        store.save_state(tmp.path(), &original).unwrap();

        // On the wire it is the bare tag — no `path`, no `gitdir`.
        let raw =
            std::fs::read_to_string(FileWorkspaceStore::mounts_json_path(tmp.path())).unwrap();
        assert!(raw.contains("\"type\": \"in-memory\""), "got: {raw}");

        let reloaded = store.load(tmp.path()).unwrap();
        assert_eq!(reloaded.mounts.len(), 2);
        // The in-memory mount round-trips back to exactly InMemory —
        // not silently reinterpreted as a folder/archive/git variant.
        let session = reloaded
            .mounts
            .iter()
            .find(|m| m.mem == "session")
            .expect("session mount survives reload");
        assert_eq!(session.storage, MountStorage::InMemory);
        // And the sibling folder mount is untouched — the two wire
        // shapes do not bleed into each other.
        let local = reloaded.mounts.iter().find(|m| m.mem == "local").unwrap();
        assert!(matches!(local.storage, MountStorage::Folder { .. }));
    }

    #[test]
    fn instantiate_lean_backend_rejects_git_branch_with_typed_error() {
        let mount = Mount {
            mem: "engine".to_string(),
            schema: Some(pin("default@1.0.0")),
            storage: MountStorage::GitBranch {
                gitdir: PathBuf::from("/some/path/.git"),
                branch: "engine".to_string(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        // `unwrap_err()` requires Box<dyn MemBackend> to be Debug;
        // matching on the Result keeps the test gix-free of that
        // bound while still asserting the typed error.
        match instantiate_lean_backend(&mount) {
            Err(InstantiateError::GitBranchRequiresMemRepoFeature { mem }) => {
                assert_eq!(mem, "engine");
            }
            Ok(_) => panic!("expected GitBranchRequiresMemRepoFeature, got Ok"),
        }
    }

    #[test]
    fn detect_layout_returns_empty_for_unrecognised_workspace() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(detect_layout(tmp.path()), Layout::Empty);
    }
    #[test]
    fn detect_layout_returns_new_when_workspace_toml_present() {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        );
        assert_eq!(detect_layout(tmp.path()), Layout::New);
    }
}
