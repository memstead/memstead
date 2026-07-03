//! Workspace concept — first-class in `memstead-base` after the
//! workspace-store rebuild.
//!
//! A [`Workspace`] is the operator-curated collection of mounts;
//! each [`Mount`] attaches one mem to the workspace via a storage
//! backend (folder / git-branch / archive). One mount = one mem:
//! five mems living on five branches in one git-repo materialise as
//! five mounts; the engine pools the gitdir handle internally for the
//! shared backend rather than collapsing the conceptual mount.
//!
//! This module ships the data shapes only. The persistence adapter
//! that materialises a `Workspace` from `.memstead/workspace.toml` +
//! `.memstead/state/mounts.json` lands separately as the file-adapter
//! sessions move forward; tests and the macOS app's in-memory builder
//! construct `Workspace` directly without going through any adapter.
//!
//! Distinct from [`crate::mem::MemRouterSnapshot`], which is the
//! engine's *runtime* snapshot of writable / visible mems. The
//! engine derives a `MemRouterSnapshot` from a `Workspace` at boot;
//! the two coexist while the rebuild is in flight.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use memstead_schema::SchemaRef;
use memstead_schema::workspace_config::CrossLinkValue;
use serde::{Deserialize, Serialize};

/// A single mem attachment in a [`Workspace`]. One mount = one
/// mem. The schema pin is on the mount because per-mem schema
/// resolution is fixed in code (local-storage → built-in → registry,
/// with the storage layer owning where "local" lives — see the
/// glossary's *Schema* entry); the mount carries which schema this
/// mem expects, the backend resolves where the YAMLs come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Operator-facing mem name within this workspace.
    pub mem: String,
    /// Optional *expectation assertion* about this mem's schema pin
    /// (exact `<name>@<version>`). The authoritative pin is the mem's
    /// own `MemConfig.schema` on its storage backend; boot/load
    /// resolve from there. This field is a workspace-local cross-check —
    /// useful for foreign or read-only mounts. `None` means "assert
    /// nothing, trust the backend config". When `Some` and mismatching
    /// the config pin, loading surfaces a `SchemaPinMismatch` finding
    /// naming both values; neither is silently preferred. Resolution
    /// falls back to this value only when the backend config carries no
    /// pin.
    pub schema: Option<SchemaRef>,
    /// Backend-specific reference to the mem's content.
    pub storage: MountStorage,
    /// Read-only or writable attachment.
    pub capability: MountCapability,
    /// Eager (open the backend at engine start) or lazy (defer to
    /// first read). V1 runtime treats every mount as `Eager`; the
    /// `Lazy` slot is reserved for archive backends that should not
    /// unzip at boot.
    pub lifecycle: MountLifecycle,
    /// Whether other mounts in the same workspace may form
    /// cross-mem edges into this mount. Workspace-level cross-mem
    /// permission policy can override.
    pub cross_linkable: bool,
    /// In-flight schema migration target. `Some(target)` puts the
    /// mem in dual-pin state: writes validate against `target`
    /// (the engine's effective validation schema), reads stay
    /// permissive, and `schema` remains the settled pin until every
    /// entity is integral against the target — then the atomic
    /// switch sets `schema = target` and clears this field in one
    /// workspace-store write. Persisted so a long migration is
    /// resumable across engine restarts.
    pub migration_target: Option<SchemaRef>,
}

impl Mount {
    /// Hierarchical organisational path for this mount, or `None` for
    /// flat layout / non-hierarchical storage. Mirrors the
    /// `MemCreateParams.path` create-side input — at delete time the
    /// lifecycle candidate composes as `<mem_path>/<name>` (or
    /// `<name>` alone when `None`) to match the create-side rule.
    ///
    /// Derivation: `MountStorage::GitBranch` carries the path in its
    /// `branch` field. Tolerates both fully-qualified
    /// `refs/heads/<mem_path>/<mem>` (the shape `create_mem`
    /// produces) and bare `<mem_path>/<mem>` (the shape
    /// `mounts.json` operator-edited entries carry) — full's
    /// `instantiate_full_backend` already normalises both forms. Strip
    /// the optional `refs/heads/` prefix and the trailing `<mem>`
    /// leaf. `Folder` / `Archive` carry no hierarchical path on the
    /// storage variant — runtime callers that know the create-time
    /// `path` plumb it directly into the router via
    /// `Engine::register_writable_mem`.
    pub fn mem_path(&self) -> Option<String> {
        match &self.storage {
            MountStorage::GitBranch { branch, .. } => {
                let leaf = branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch.as_str());
                let after_leaf = leaf.strip_suffix(&self.mem)?;
                let trimmed = after_leaf.trim_end_matches('/');
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            MountStorage::Folder { .. } | MountStorage::Archive { .. } | MountStorage::InMemory => {
                None
            }
        }
    }
}

/// Storage reference for a [`Mount`]. One variant per
/// [`crate::backend::MemBackend`] implementation. New backends add
/// a variant; the file-adapter learns to round-trip it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountStorage {
    /// Folder backend — mem lives as a directory tree on disk.
    /// The mem root may be the workspace root itself (collapsed
    /// single-mem form: `.memstead/config.json` at root, no `mems/`
    /// subfolder) or a sibling mem subfolder.
    Folder {
        /// Absolute path to the mem root directory.
        path: PathBuf,
    },
    /// Git-branch backend — mem lives as a branch in a mem-repo
    /// gitdir. Multi-repo workspaces are supported by varying
    /// `gitdir` across mounts (see the *Storage backend* glossary
    /// entry's *per-mount git-repo* block for the trade-offs).
    GitBranch {
        /// Absolute path to the gitdir
        /// (typically `<workspace>/mem-repo/.git`).
        gitdir: PathBuf,
        /// Branch name within the gitdir holding the mem content.
        branch: String,
    },
    /// Archive backend — mem lives inside a sealed `.mem` zip archive.
    /// Always read-only; mounts of this storage carry
    /// [`MountCapability::ReadOnly`].
    Archive {
        /// Absolute path to the sealed archive file.
        path: PathBuf,
    },
    /// In-memory backend — mem lives entirely in RAM, with no
    /// filesystem path and no git. Created empty, dropped with the
    /// engine, leaving no on-disk residue. Serves ephemeral
    /// per-session playground mems. Carries no fields: there is
    /// nothing to locate on disk, and the backend holds all state
    /// itself (see [`crate::storage::InMemoryBackend`]).
    InMemory,
}

impl MountStorage {
    /// Stable kebab-case backend identifier surfaced in error envelopes
    /// (e.g. `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND`'s `active_backend`
    /// detail) and in the on-disk `mounts.json` serialisation. The
    /// kebab-case form matches the `MountStorageWire` `#[serde(tag,
    /// rename_all = "kebab-case")]` tag.
    pub fn backend_id(&self) -> &'static str {
        match self {
            MountStorage::Folder { .. } => "folder",
            MountStorage::GitBranch { .. } => "git-branch",
            MountStorage::Archive { .. } => "archive",
            MountStorage::InMemory => "in-memory",
        }
    }

    /// Whether writes (and, for read-only backends, the loaded content)
    /// survive process restart / session-TTL eviction. `Folder`,
    /// `GitBranch`, and `Archive` all live on disk and persist; only
    /// `InMemory` is volatile — its state is dropped with the engine,
    /// so a `commit_sha` it returns denotes nothing durable. This is the
    /// fact the durability marker projects: derived from the storage
    /// *kind*, not from `current_head()` (which is `None` for both
    /// `Folder` and `InMemory` and so cannot tell them apart).
    pub fn is_durable(&self) -> bool {
        match self {
            MountStorage::Folder { .. }
            | MountStorage::GitBranch { .. }
            | MountStorage::Archive { .. } => true,
            MountStorage::InMemory => false,
        }
    }
}

/// What the workspace may do with a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountCapability {
    /// Mutations rejected — the engine surfaces a typed read-only
    /// error before reaching the backend.
    ReadOnly,
    /// Full read + write.
    Write,
}

/// When the mount's backend initialises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountLifecycle {
    /// Open the backend at engine start.
    Eager,
    /// Defer initialisation until first read. V1 runtime ignores
    /// this and treats every mount as `Eager`; the slot is reserved
    /// for archive-backed mounts that should not unzip at boot.
    Lazy,
}

/// Operator-curated workspace — the in-memory shape the engine
/// receives.
///
/// The two-layer file adapter produces a `Workspace` by reading
/// `.memstead/workspace.toml` (operator-edited rules) and
/// `.memstead/state/mounts.json` (engine-managed mount list). Tests and
/// the macOS app's in-memory builder construct `Workspace` directly.
///
/// V1 carries the mount list and operator policy. Plugin hooks and
/// pipeline-config handles attach as additive fields — no breaking
/// changes expected.
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    pub mounts: Vec<Mount>,
    /// Workspace-level operator policy (mem create/delete rules,
    /// cross-mem link permissions). Defaults to empty for tests
    /// and the macOS app's in-memory builder; the file adapter
    /// populates from `.memstead/workspace.toml`'s `[mem_management]`
    /// and `[cross_mem_links]` sections. The unified engine reads
    /// this via [`crate::Engine::settings`] after
    /// [`crate::Engine::from_workspace_root`] threads it through
    /// [`crate::Engine::set_settings`].
    pub settings: WorkspaceSettings,
}

impl Workspace {
    /// Empty workspace — zero mounts, default settings. Useful for
    /// tests; production workspaces always carry at least one mount
    /// (the engine rejects an empty `Workspace` at boot).
    pub fn empty() -> Self {
        Self {
            mounts: Vec::new(),
            settings: WorkspaceSettings::default(),
        }
    }
}

/// Workspace-level operator policy carried alongside the mount list.
///
/// Data carriers only — the matcher compilation lives in
/// `crate::mem_management::CreateRuleSet`. The engine carries the
/// raw settings so MCP handlers can surface them under `memstead_health
/// { include_config: true }` and `memstead_overview`'s
/// lifecycle-namespaces section.
///
/// `Default::default()` is a totally-empty policy: zero create rules,
/// zero delete rules, no cross-mem link policy. The unified engine
/// uses this as the bootstrap value at construction time; consumers
/// that load a real policy call [`crate::Engine::set_settings`].
#[derive(Debug, Clone, Default)]
pub struct WorkspaceSettings {
    /// Raw `[[mem_management.create]]` rules in declaration order.
    /// Each entry carries a gitignore-style `pattern` matched against
    /// the candidate mem path, an `schemas[]` allowlist, and an
    /// optional `default_cross_links` synthesised cross-link
    /// permission. Empty list means "no agent-driven mem creation
    /// allowed" — `memstead_mem_create` rejects every candidate.
    pub mem_create_rules: Vec<CreateRuleSetting>,
    /// Raw `[[mem_management.delete]]` rules. Same first-match
    /// semantics as [`Self::mem_create_rules`], minus the schema
    /// dimension. Empty list means "no agent-driven mem deletion
    /// allowed".
    pub mem_delete_rules: Vec<DeleteRuleSetting>,
    /// `[cross_mem_links]` policy — workspace-level cross-mem
    /// edge permissions keyed by source mem. Empty map means
    /// default-deny: every cross-mem edge fails until at least one
    /// matching entry exists or a create-rule synthesised one.
    pub cross_mem_links: BTreeMap<String, CrossLinkValue>,
    /// `[mcp]` section — MCP-binary tuning knobs that operators set
    /// per-workspace. The MCP binary reads this off
    /// `Engine::settings()` at boot to size the response chunker
    /// (`token_budget`) and filter the advertised tool surface
    /// (`disabled_tools`). Defaulted when the section is absent.
    pub mcp: McpSection,
    /// `[mutations]` section — engine-wide mutation policy. The
    /// `require_notes` field surfaces a `WarningHint::NoteMissing` on
    /// mutation calls that omit a `note`. Default-zeroed when absent.
    pub mutations: MutationsSection,
    /// `[plugin.*]` namespace — opaque pass-through map keyed by
    /// plugin identifier (`claude_code`, `macos`, …). Values are raw
    /// TOML tables the engine never inspects; named plugins read
    /// their own sub-table via `memstead_health { include_config: true }`.
    pub plugin: HashMap<String, toml::Table>,
}

/// `[mcp]` section — settings the MCP binary reads at boot. Carried
/// on `WorkspaceSettings` so the MCP server sources its tuning from
/// `Engine::settings()` instead of a parallel TOML parse.
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpSection {
    /// Per-response chunking budget in tokens. `None` → caller falls
    /// back to the compile-time `DEFAULT_TOKEN_BUDGET`.
    pub token_budget: Option<usize>,
    /// Blocklist of tool names. Entries matching a compiled-in tool
    /// are hidden from `tools/list` and rejected with `TOOL_DISABLED`
    /// on direct invocation. Unknown entries log a warning and drop
    /// from the effective set. Empty / absent → every compiled-in
    /// tool is advertised.
    pub disabled_tools: Option<Vec<String>>,
}

/// `[mutations]` section — engine-wide mutation policy. Carried on
/// `WorkspaceSettings` so plugins can read the configured posture via
/// `memstead_health { include_config: true }` without a round-trip.
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MutationsSection {
    /// When `true`, a mutation call without a `note` field emits a
    /// `WarningHint { code: "note_missing" }`. The mutation still
    /// succeeds — provenance is best-effort.
    pub require_notes: Option<bool>,
}

/// One `[[mem_management.create]]` rule. Carries a glob `pattern`
/// matched against the candidate mem path, the `schemas` allowlist
/// (each entry an exact `name@x.y.z` pin or the literal `"*"` for
/// any-schema), and an optional `default_cross_links` value applied
/// to every mem the rule matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRuleSetting {
    pub pattern: String,
    pub schemas: Vec<String>,
    pub default_cross_links: Option<CrossLinkValue>,
}

/// One `[[mem_management.delete]]` rule. Carries only a `pattern`;
/// delete has no schema dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteRuleSetting {
    pub pattern: String,
}

/// The literal `"*"` schema-allowlist entry that admits any pinned
/// schema. Consumed by the create-rule allowlist parser.
pub const SCHEMA_WILDCARD: &str = "*";

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(name: &str) -> SchemaRef {
        SchemaRef::new(name, semver::Version::new(1, 0, 0))
    }

    #[test]
    fn empty_workspace_has_no_mounts() {
        let ws = Workspace::empty();
        assert!(ws.mounts.is_empty());
    }

    #[test]
    fn durability_follows_storage_kind() {
        // On-disk backends persist; only in-memory is volatile. This is
        // the fact the durability marker projects across overview / health
        // / mutation responses.
        let folder = MountStorage::Folder {
            path: PathBuf::from("/work/mem"),
        };
        let git = MountStorage::GitBranch {
            gitdir: PathBuf::from("/work/mem-repo/.git"),
            branch: "specs".into(),
        };
        let archive = MountStorage::Archive {
            path: PathBuf::from("/work/curated.mem"),
        };
        let in_memory = MountStorage::InMemory;

        assert!(folder.is_durable());
        assert!(git.is_durable());
        assert!(archive.is_durable());
        assert!(!in_memory.is_durable());

        // The backend_id kebab string the marker rides alongside.
        assert_eq!(folder.backend_id(), "folder");
        assert_eq!(git.backend_id(), "git-branch");
        assert_eq!(archive.backend_id(), "archive");
        assert_eq!(in_memory.backend_id(), "in-memory");
    }

    #[test]
    fn mount_can_describe_folder_storage() {
        let m = Mount {
            mem: "specs".into(),
            schema: Some(pin("default")),
            storage: MountStorage::Folder {
                path: PathBuf::from("/work/mem"),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        assert_eq!(m.mem, "specs");
        assert!(matches!(m.storage, MountStorage::Folder { .. }));
    }

    #[test]
    fn mount_can_describe_git_branch_storage() {
        let m = Mount {
            mem: "engine".into(),
            schema: Some(pin("default")),
            storage: MountStorage::GitBranch {
                gitdir: PathBuf::from("/work/mem-repo/.git"),
                branch: "engine".into(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        assert!(matches!(m.storage, MountStorage::GitBranch { .. }));
    }

    /// `Mount::mem_path()` derives the hierarchical path component
    /// the delete-side lifecycle composer needs. Tolerates both bare
    /// `<path>/<mem>` (operator-edited mounts.json) and
    /// fully-qualified `refs/heads/<path>/<mem>` (runtime-created
    /// mems). Folder / Archive variants always return `None`.
    #[test]
    fn mem_path_extracts_hierarchical_prefix_from_git_branch() {
        // Bare hierarchical (mounts.json shape).
        let m = Mount {
            mem: "engine".into(),
            schema: Some(pin("default")),
            storage: MountStorage::GitBranch {
                gitdir: PathBuf::from("/work/mem-repo/.git"),
                branch: "memstead/engine".into(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        assert_eq!(m.mem_path(), Some("memstead".to_string()));

        // Fully-qualified hierarchical (create_mem shape).
        let m = Mount {
            mem: "plan-foo".into(),
            schema: Some(pin("default")),
            storage: MountStorage::GitBranch {
                gitdir: PathBuf::from("/work/mem-repo/.git"),
                branch: "refs/heads/planning/plan-foo".into(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        assert_eq!(m.mem_path(), Some("planning".to_string()));

        // Multi-segment hierarchical prefix.
        let m = Mount {
            mem: "leaf".into(),
            schema: Some(pin("default")),
            storage: MountStorage::GitBranch {
                gitdir: PathBuf::from("/work/mem-repo/.git"),
                branch: "refs/heads/a/b/c/leaf".into(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        assert_eq!(m.mem_path(), Some("a/b/c".to_string()));

        // Flat layout (bare leaf, no prefix).
        let m = Mount {
            mem: "engine".into(),
            schema: Some(pin("default")),
            storage: MountStorage::GitBranch {
                gitdir: PathBuf::from("/work/mem-repo/.git"),
                branch: "engine".into(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        assert_eq!(m.mem_path(), None);

        // Flat layout (fully-qualified, no prefix beyond refs/heads/).
        let m = Mount {
            mem: "engine".into(),
            schema: Some(pin("default")),
            storage: MountStorage::GitBranch {
                gitdir: PathBuf::from("/work/mem-repo/.git"),
                branch: "refs/heads/engine".into(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        assert_eq!(m.mem_path(), None);

        // Folder backend has no hierarchical concept.
        let m = Mount {
            mem: "engine".into(),
            schema: Some(pin("default")),
            storage: MountStorage::Folder {
                path: PathBuf::from("/work/mem"),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        assert_eq!(m.mem_path(), None);
    }

    #[test]
    fn mount_can_describe_archive_storage() {
        let m = Mount {
            mem: "external".into(),
            schema: Some(pin("default")),
            storage: MountStorage::Archive {
                path: PathBuf::from("/deps/external.mem"),
            },
            capability: MountCapability::ReadOnly,
            lifecycle: MountLifecycle::Lazy,
            cross_linkable: false,
            migration_target: None,
        };
        assert!(matches!(m.storage, MountStorage::Archive { .. }));
        assert_eq!(m.capability, MountCapability::ReadOnly);
    }

    #[test]
    fn workspace_with_heterogeneous_mounts() {
        let ws = Workspace {
            mounts: vec![
                Mount {
                    mem: "engine".into(),
                    schema: Some(pin("default")),
                    storage: MountStorage::GitBranch {
                        gitdir: PathBuf::from("/work/mem-repo/.git"),
                        branch: "engine".into(),
                    },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: true,
                    migration_target: None,
                },
                Mount {
                    mem: "macos".into(),
                    schema: Some(pin("default")),
                    storage: MountStorage::GitBranch {
                        gitdir: PathBuf::from("/work/mem-repo/.git"),
                        branch: "macos".into(),
                    },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: true,
                    migration_target: None,
                },
                Mount {
                    mem: "external".into(),
                    schema: Some(pin("default")),
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
        assert_eq!(ws.mounts.len(), 3);
        // Two mounts share a gitdir — the engine will pool the handle
        // internally; the conceptual mount stays per-mem.
        let shared_gitdir_mounts = ws
            .mounts
            .iter()
            .filter(|m| matches!(&m.storage, MountStorage::GitBranch { gitdir, .. } if gitdir == std::path::Path::new("/work/mem-repo/.git")))
            .count();
        assert_eq!(shared_gitdir_mounts, 2);
    }
}
