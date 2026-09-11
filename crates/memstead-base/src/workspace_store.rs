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
//! [`instantiate_local_backend`] for folder + archive variants. The
//! git-branch backend lives in the `memstead-git-branch` crate and is
//! injected through the engine's backend factory; an engine without that
//! factory cannot materialise a `MountStorage::GitBranch` mount and surfaces
//! [`InstantiateError::GitBranchBackendUnavailable`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backend::MemBackend;
use crate::storage::{ArchiveBackend, FilesystemBackend, InMemoryBackend};
use crate::workspace::{
    McpSection, Mount, MountCapability, MountLifecycle, MountStorage, MutationsSection, Workspace,
    WorkspaceSettings,
};

/// The engine-managed workspace store directory under the workspace
/// root — `<workspace_root>/.memstead/` holds `workspace.toml` and
/// `state/mounts.json`, the roster of everything this workspace
/// mounts. (It also carries an empty `memstead-io/` directory the mem
/// initialiser seeds; nothing reads it since the tier-3 archive
/// resolver was removed on 2026-08-27, and retiring the directory
/// itself is a separate change to what `init` creates.) Distinct from
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
    #[error("workspace store not found at {path} — run `memstead mem-repo init` first")]
    NotInitialised { path: PathBuf },
    /// IO failure reading or writing one of the adapter files.
    #[error(
        "workspace store io error at {path}: {source} — no memstead command repairs this; \
         check filesystem permissions and disk state"
    )]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// TOML or JSON parse / serialise failure. The wrapped string is
    /// the underlying serde error; the file is named so operators
    /// know where to look.
    #[error(
        "workspace store parse error at {path}: {message} — no memstead command repairs this; \
         fix the named file by hand or restore it from version control"
    )]
    Parse { path: PathBuf, message: String },
    /// Format version mismatch — adapter understands a different
    /// schema version than the file declares.
    #[error(
        "workspace store format mismatch at {path}: expected {expected}, found {found} — \
         no memstead command repairs this; use an engine version whose format matches the file"
    )]
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
    /// A `projections/` directory holds a file in a retired binding format:
    /// either a version-less (gen-2 four-primitive) projection or a v1
    /// binding of the retired three-file store. The loader serves only v2
    /// (one record per pipeline) and the engine no longer converts the
    /// retired formats; the binding is re-authored with `memstead projection
    /// init`. [`StoreError::code`] maps this to the `PROJECTION_STORE_LEGACY`
    /// token on every surface.
    #[error(
        "retired binding format at {path}: this file is a gen-2 (version-less) or v1 binding, \
         a format this engine no longer converts; re-author the binding with \
         `memstead projection init`"
    )]
    LegacyProjectionStore { path: PathBuf },
    /// A binding file declares a `version` the loader does not understand
    /// (only v2 = `2` is supported; v1 and version-less files surface
    /// [`Self::LegacyProjectionStore`] instead). Refused, never reinterpreted.
    #[error(
        "unsupported binding format version {version} at {path}: this engine understands v2 (version 2)"
    )]
    UnknownBindingVersion { path: PathBuf, version: i64 },
    /// Catch-all for adapter-specific failures. Carries an
    /// agent-readable message; structured variants extend the enum.
    #[error("workspace store error: {0}")]
    Other(String),
}

impl StoreError {
    /// Stable, surface-independent error code token, following the
    /// [`crate::EngineError::code`] convention (UPPER_SNAKE). Boot
    /// failures route through [`crate::engine::BootError::code`],
    /// which delegates here — a store-layer failure carries the same
    /// typed code on CLI stderr, `--json` envelopes, and the MCP
    /// server's boot diagnostics.
    pub fn code(&self) -> &'static str {
        match self {
            // Same token the CLI's setup layer uses for "no workspace
            // marker found" — one condition, one code, regardless of
            // whether the walk or the store load detected it.
            StoreError::NotInitialised { .. } => "WORKSPACE_NOT_INITIALISED",
            StoreError::Io { .. } => "WORKSPACE_STORE_IO",
            StoreError::Parse { .. } => "WORKSPACE_STORE_PARSE",
            StoreError::FormatMismatch { .. } => "WORKSPACE_STORE_FORMAT_MISMATCH",
            StoreError::LegacyLayout { .. } => "LEGACY_WORKSPACE_LAYOUT",
            StoreError::LegacyProjectionStore { .. } => "PROJECTION_STORE_LEGACY",
            StoreError::UnknownBindingVersion { .. } => "UNKNOWN_BINDING_VERSION",
            StoreError::Other(_) => "WORKSPACE_STORE_ERROR",
        }
    }
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
    ///
    /// Last-writer-wins. Prefer [`Self::save_state_cas`] whenever the
    /// caller can name what it read: a long-lived process that dumps
    /// its cached roster over this call drops every mount a sibling
    /// process registered since the dump was taken.
    fn save_state(&self, workspace_root: &Path, workspace: &Workspace) -> Result<(), StoreError>;

    /// Raw bytes of the engine-managed state file, or `None` when it
    /// does not exist yet. The compare token for
    /// [`Self::save_state_cas`].
    fn read_state_bytes(&self, workspace_root: &Path) -> Result<Option<Vec<u8>>, StoreError>;

    /// Parse state bytes previously returned by
    /// [`Self::read_state_bytes`] into the mount roster they carry.
    fn parse_state_bytes(
        &self,
        workspace_root: &Path,
        bytes: &[u8],
    ) -> Result<Vec<Mount>, StoreError>;

    /// Compare-and-set counterpart of [`Self::save_state`]: write only
    /// when the on-disk state still equals `expected`, and report a
    /// mismatch as `Ok(false)` rather than an error so the caller can
    /// re-read, re-merge and retry. The check and the write are one
    /// step under the adapter's own lock — a caller that reads, then
    /// compares, then writes leaves exactly the window this exists to
    /// close.
    fn save_state_cas(
        &self,
        workspace_root: &Path,
        workspace: &Workspace,
        expected: Option<&[u8]>,
    ) -> Result<bool, StoreError>;
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

/// True when the workspace root carries `mem-repo/.git/` — the
/// multi-mem, git-backed shape. False for the single-mem, history-free
/// filesystem-mem shape. The `.memstead/workspace.toml` marker is
/// shape-neutral, so this directory probe is what distinguishes the two.
///
/// The shared shape primitive: the CLI's `WorkspaceShape` resolution,
/// the mem-repo-only refusals, and the MCP boot line all route through
/// here so no surface can name a shape the others disagree with.
pub fn is_mem_repo_shaped(workspace_root: &Path) -> bool {
    workspace_root.join("mem-repo").join(".git").is_dir()
}

/// The one spelling of a workspace's shape, for any surface that names
/// it to a human or an agent: `"mem-repo"` or `"filesystem-mem"`. Both
/// spellings match the vocabulary the refusals use
/// (`UNSUPPORTED_WORKSPACE_SHAPE`) and the commands that create each
/// shape (`memstead mem-repo init` / `memstead quickstart`).
pub fn workspace_shape_label(workspace_root: &Path) -> &'static str {
    if is_mem_repo_shaped(workspace_root) {
        "mem-repo"
    } else {
        "filesystem-mem"
    }
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
            Ok(text) => parse_mounts_text(&text, &mounts_path, workspace_root)?,
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
        ensure_state_dir(&mounts_path)?;
        let text = render_mounts_text(workspace, workspace_root, &mounts_path)?;
        std::fs::write(&mounts_path, text).map_err(|e| StoreError::Io {
            path: mounts_path,
            source: e,
        })?;
        Ok(())
    }

    fn read_state_bytes(&self, workspace_root: &Path) -> Result<Option<Vec<u8>>, StoreError> {
        let mounts_path = Self::mounts_json_path(workspace_root);
        match std::fs::read(&mounts_path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Io {
                path: mounts_path,
                source: e,
            }),
        }
    }

    fn parse_state_bytes(
        &self,
        workspace_root: &Path,
        bytes: &[u8],
    ) -> Result<Vec<Mount>, StoreError> {
        let mounts_path = Self::mounts_json_path(workspace_root);
        let text = std::str::from_utf8(bytes).map_err(|e| StoreError::Parse {
            path: mounts_path.clone(),
            message: e.to_string(),
        })?;
        parse_mounts_text(text, &mounts_path, workspace_root)
    }

    fn save_state_cas(
        &self,
        workspace_root: &Path,
        workspace: &Workspace,
        expected: Option<&[u8]>,
    ) -> Result<bool, StoreError> {
        let mounts_path = Self::mounts_json_path(workspace_root);
        ensure_state_dir(&mounts_path)?;
        let text = render_mounts_text(workspace, workspace_root, &mounts_path)?;

        // Same lockfile shape the folder backend's config compare-and-set
        // uses: hold an exclusive marker, compare, write, release on every
        // exit path. 50 x 10ms, then the holder is presumed dead — a state
        // write is milliseconds of work, so half a second of contention is
        // not a busy writer.
        let lock_path = mounts_path.with_extension("json.lock");
        let mut held = None;
        for attempt in 0..50 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(f) => {
                    held = Some(f);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 49 {
                        let _ = std::fs::remove_file(&lock_path);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => {
                    return Err(StoreError::Io {
                        path: lock_path,
                        source: e,
                    });
                }
            }
        }
        let _lock = match held {
            Some(f) => f,
            None => std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&lock_path)
                .map_err(|e| StoreError::Io {
                    path: lock_path.clone(),
                    source: e,
                })?,
        };
        let release = || {
            let _ = std::fs::remove_file(&lock_path);
        };

        let current = match std::fs::read(&mounts_path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                release();
                return Err(StoreError::Io {
                    path: mounts_path,
                    source: e,
                });
            }
        };
        if current.as_deref() != expected {
            release();
            return Ok(false);
        }
        let result = std::fs::write(&mounts_path, text).map_err(|e| StoreError::Io {
            path: mounts_path,
            source: e,
        });
        release();
        result.map(|_| true)
    }
}

/// Create the `state/` directory the mounts file lives in.
fn ensure_state_dir(mounts_path: &Path) -> Result<(), StoreError> {
    if let Some(parent) = mounts_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| StoreError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    Ok(())
}

/// Render a workspace's mount roster as `mounts.json` text.
fn render_mounts_text(
    workspace: &Workspace,
    workspace_root: &Path,
    mounts_path: &Path,
) -> Result<String, StoreError> {
    let doc = MountsJsonDoc {
        format: MOUNTS_JSON_FORMAT_V3.to_string(),
        mounts: workspace
            .mounts
            .iter()
            .map(|m| MountWire::from_mount(m, workspace_root))
            .collect(),
    };
    serde_json::to_string_pretty(&doc).map_err(|e| StoreError::Parse {
        path: mounts_path.to_path_buf(),
        message: e.to_string(),
    })
}

/// Parse `mounts.json` text into the roster it carries, enforcing the
/// format pin. Probes the format field before the full parse: a
/// pre-rename file fails record deserialisation (old unit-noun field
/// name), and the typed `LegacyLayout` refusal must win over that
/// generic parse error.
fn parse_mounts_text(
    text: &str,
    mounts_path: &Path,
    workspace_root: &Path,
) -> Result<Vec<Mount>, StoreError> {
    let probe: MountsFormatProbe = serde_json::from_str(text).map_err(|e| StoreError::Parse {
        path: mounts_path.to_path_buf(),
        message: e.to_string(),
    })?;
    if MOUNTS_JSON_FORMAT_LEGACY.contains(&probe.format.as_str()) {
        return Err(StoreError::LegacyLayout {
            path: mounts_path.to_path_buf(),
            found: probe.format,
        });
    }
    if probe.format != MOUNTS_JSON_FORMAT_V3 {
        return Err(StoreError::FormatMismatch {
            path: mounts_path.to_path_buf(),
            expected: MOUNTS_JSON_FORMAT_V3.to_string(),
            found: probe.format,
        });
    }
    let doc: MountsJsonDoc = serde_json::from_str(text).map_err(|e| StoreError::Parse {
        path: mounts_path.to_path_buf(),
        message: e.to_string(),
    })?;
    Ok(doc
        .mounts
        .into_iter()
        .map(|w| w.into_mount(workspace_root))
        .collect())
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

/// Errors surfaced by [`instantiate_local_backend`].
#[derive(Debug, thiserror::Error)]
pub enum InstantiateError {
    /// Mount declares a `MountStorage::GitBranch` storage variant but
    /// `memstead-base` alone cannot construct a git-branch backend —
    /// the implementation lives in the `memstead-git-branch` crate.
    ///
    /// The remedy text names the two real, published entry points and
    /// nothing else: an earlier revision pointed at a `mem-repo` Cargo
    /// feature that does not exist on the published crates and named
    /// `instantiate_full_backend` without saying how to wire it —
    /// a cold-start run burned its longest wall on exactly that
    /// (protocol 0-8-0, F6). Keep this message in lockstep with the
    /// actual `memstead-git-branch` public API.
    #[error(
        "mem {mem}: git-branch storage needs the memstead-git-branch crate \
         (`cargo add memstead-git-branch`). Simplest: open the workspace with \
         `memstead_git_branch::workspace_store::engine_from_workspace_root(root)` \
         instead of the memstead-base constructor. Alternative, if you build the \
         Engine yourself: `engine.set_backend_factory(\
         memstead_git_branch::storage::instantiate_full_backend)` before mounting"
    )]
    GitBranchBackendUnavailable { mem: String },
}

impl InstantiateError {
    /// Stable, surface-independent error code token (UPPER_SNAKE, per
    /// the [`crate::EngineError::code`] convention). Reuses the CLI's
    /// existing `UNSUPPORTED_WORKSPACE_SHAPE` token: both fire when an
    /// engine without the git-branch factory meets a git-branch-shaped
    /// workspace, and the
    /// agent's next step is identical.
    pub fn code(&self) -> &'static str {
        match self {
            InstantiateError::GitBranchBackendUnavailable { .. } => "UNSUPPORTED_WORKSPACE_SHAPE",
        }
    }
}

/// Materialise a [`MemBackend`] for `mount` using the local
/// backends (folder + archive). Returns an error for the git-branch
/// variant; `memstead-git-branch` handles that with its
/// `instantiate_full_backend` counterpart, installed through the backend
/// factory.
///
/// Lives in `memstead-base` because both folder and archive backends are
/// always-on; the function shape (one mount in, one boxed backend
/// out) stays uniform for both factories so the engine's
/// `from_mounts` glue is identical with and without the git-branch crate.
pub fn instantiate_local_backend(mount: &Mount) -> Result<Box<dyn MemBackend>, InstantiateError> {
    match &mount.storage {
        MountStorage::Folder { path } => Ok(Box::new(FilesystemBackend::new(path.clone()))),
        MountStorage::Archive { path } => Ok(Box::new(ArchiveBackend::new(path.clone()))),
        MountStorage::InMemory => Ok(Box::new(InMemoryBackend::new())),
        MountStorage::GitBranch { .. } => Err(InstantiateError::GitBranchBackendUnavailable {
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
mod tests;
