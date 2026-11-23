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
//! sessions move forward; tests and in-memory builders construct
//! `Workspace` directly without going through any adapter.
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
    /// Eager (open the backend at engine start) or lazy (entity load
    /// deferred to first read) — see [`MountLifecycle`] for the full
    /// contract. Opt-in per mount.
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

/// Branch name of the mem-repo's unified engine ref — schemas plus
/// per-mem configs in one tree. Not a mem: it has no mount, and the
/// transport verbs that walk every ref (`push --all`) carry it by
/// name.
pub const MEMSTEAD_REF_BRANCH: &str = "__MEMSTEAD";

/// Branch name of the mem-repo's check-ledger transport ref: one tree
/// with `mems/<mem>/checks.jsonl` per mem, the mem's rows of the
/// workspace check ledger as last published. Not a mem and not the
/// ledger's home: the ledger stays the workspace file, and this ref
/// is how its rows travel between machines with `push`, `fetch` and
/// `pull` (the engine unions rows, never rewrites them). Never
/// mounted; `status --remote` does not classify it.
pub const MEMSTEAD_CHECKS_REF_BRANCH: &str = "__MEMSTEAD_CHECKS";

/// The path of one mem's ledger rows on the checks transport ref.
pub fn checks_ref_member_path(mem: &str) -> String {
    format!("mems/{mem}/checks.jsonl")
}

/// A mount's declared branch as a fully-qualified local ref. The
/// `branch` field tolerates both `refs/heads/<path>` (used verbatim,
/// any `refs/` value is) and bare `<path>` (prefixed) — the same
/// normalisation `instantiate_full_backend` applies. Every operation
/// that needs the mem's local ref derives it from the declared branch
/// through this function; nothing reconstructs a ref from the mem
/// name.
pub fn branch_full_ref(branch: &str) -> String {
    if branch.starts_with("refs/") {
        branch.to_string()
    } else {
        format!("refs/heads/{branch}")
    }
}

/// A mount's declared branch as the short name remote-tracking refs
/// use (`refs/remotes/<remote>/<short>`): the `refs/heads/` prefix
/// stripped when present, the value verbatim otherwise.
pub fn branch_short_name(branch: &str) -> &str {
    branch.strip_prefix("refs/heads/").unwrap_or(branch)
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
    /// so a `write_id` it returns denotes nothing durable. This is the
    /// fact the durability marker projects: derived from the storage
    /// *kind*, not from `current_head()` (which is `None` for both
    /// `Folder` and `InMemory` and so cannot tell them apart).
    /// How the durability answer for this storage was arrived at.
    ///
    /// [`Self::is_durable`] answers from the storage KIND, which is a real
    /// answer to a narrow question (does a write survive process restart)
    /// and is routinely read as a broader one (is the write recorded
    /// somewhere it could be recovered from). Callers cannot tell the two
    /// apart from a bare boolean, so the basis travels with it.
    /// The marker itself is unchanged and stays.
    pub fn durability_basis(&self, head: Option<&str>) -> DurabilityBasis {
        match self {
            // A real commit object, named by a backend that HAS commits. The
            // storage kind gates this on purpose: a folder backend's
            // `current_head()` is its change ledger's last timestamp, not a
            // commit, so keying only on "a head exists" reported `established`
            // for every folder mem that had ever been written — strictly
            // stronger than the mount-kind answer this field was added to
            // qualify, which is the misreading it exists to prevent.
            MountStorage::GitBranch { .. } if head.is_some_and(|h| !h.is_empty()) => {
                DurabilityBasis::Established
            }
            _ => DurabilityBasis::InferredFromMountKind,
        }
    }

    pub fn is_durable(&self) -> bool {
        match self {
            MountStorage::Folder { .. }
            | MountStorage::GitBranch { .. }
            | MountStorage::Archive { .. } => true,
            MountStorage::InMemory => false,
        }
    }
}

/// Whether a durability answer was established or inferred.
///
/// The distinction exists because the engine's answer is derived from the
/// mount kind, which is honest about surviving a restart and says nothing
/// about the write having reached version control. A folder mem is the case
/// that matters: its writes land on disk, so `durable` is true, and whether
/// anything could recover them is a question the engine cannot answer at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DurabilityBasis {
    /// The backend named a real commit for this write, so it is recorded.
    Established,
    /// Read off the storage kind. True for surviving a restart; silent on
    /// whether the write is recorded anywhere it could be recovered from.
    InferredFromMountKind,
}

impl DurabilityBasis {
    pub fn as_wire(&self) -> &'static str {
        match self {
            DurabilityBasis::Established => "established",
            DurabilityBasis::InferredFromMountKind => "inferred-from-mount-kind",
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
    /// Defer the ENTITY load until the first operation that needs the
    /// mem. The metadata half (config, provenance, schema pin) still
    /// resolves at boot, so the mem is on the roster with its pin —
    /// present, never silently absent — and a broken pin quarantines at
    /// boot exactly as an eager mount's would. The first read triggers
    /// the load through the per-operation funnel
    /// (`Engine::ensure_mems_loaded`, called by `reload_if_stale`),
    /// running the same validation gauntlet an eager boot runs; a load
    /// failure quarantines at that moment with the same typed
    /// reporting. Opt-in per mount; nothing sets it by default.
    Lazy,
}

/// Operator-curated workspace — the in-memory shape the engine
/// receives.
///
/// The two-layer file adapter produces a `Workspace` by reading
/// `.memstead/workspace.toml` (operator-edited rules) and
/// `.memstead/state/mounts.json` (engine-managed mount list). Tests and
/// in-memory builders construct `Workspace` directly.
///
/// V1 carries the mount list and operator policy. Plugin hooks and
/// pipeline-config handles attach as additive fields — no breaking
/// changes expected.
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    pub mounts: Vec<Mount>,
    /// Workspace-level operator policy (mem create/delete rules,
    /// cross-mem link permissions). Defaults to empty for tests
    /// and in-memory builders; the file adapter
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
    /// plugin identifier (`claude_code`, …). Values are raw
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
    /// How many seconds a write waits for a mem's write lock before it
    /// refuses `WRITE_LOCK_TIMEOUT`. The same limit bounds the wait of
    /// a load, and of a direct read, of a folder mem that holds a
    /// commit which is decided and not applied while another process
    /// holds the mem's write lock: past it the mem is not served and
    /// the condition is `COMMIT_NOT_APPLIED`. Absent → 30. `0` waits
    /// without bound. A value that is not a non-negative whole number
    /// refuses when the configuration is loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_lock_timeout_secs: Option<u64>,
}

impl MutationsSection {
    /// The default wait for a mem's write lock, in seconds.
    pub const DEFAULT_WRITE_LOCK_TIMEOUT_SECS: u64 = 30;

    /// The limit a write waits for a mem's write lock: `None` waits
    /// without bound (the key set to `0`), otherwise the configured
    /// seconds or the default.
    pub fn write_lock_timeout(&self) -> Option<std::time::Duration> {
        match self
            .write_lock_timeout_secs
            .unwrap_or(Self::DEFAULT_WRITE_LOCK_TIMEOUT_SECS)
        {
            0 => None,
            secs => Some(std::time::Duration::from_secs(secs)),
        }
    }
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

/// The git directory `mount` lives in, when that is not the workspace's
/// own `mem-repo/.git`: a branch of another workspace's mem-repo, or
/// any git-branch mount of a workspace that has no root or no mem-repo
/// of its own. `None` for a mount of the workspace's own mem-repo and
/// for every other storage. Compared after canonicalisation.
pub fn lives_in_another_git_directory<'a>(
    mount: &'a Mount,
    workspace_root: Option<&std::path::Path>,
) -> Option<&'a std::path::Path> {
    let MountStorage::GitBranch { gitdir, .. } = &mount.storage else {
        return None;
    };
    let canonical = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let own = workspace_root.map(|root| root.join("mem-repo").join(".git"));
    own.is_none_or(|own| canonical(&own) != canonical(gitdir))
        .then_some(gitdir.as_path())
}

/// Refuse a pin switch whose target the mem's own git directory cannot
/// resolve. A pin is written into the configuration that lives with
/// the mem; for a mem mounted from another git directory that is
/// another workspace's registry, and a schema only the asking
/// workspace holds would leave the mem unresolvable wherever else it
/// is mounted. A built-in target and a target installed in that
/// directory pass; `registry_holds` answers the latter for the
/// directory it is given. A mount of the workspace's own mem-repo and
/// every other storage pass unasked.
pub fn pin_is_held_where_the_mem_lives(
    mount: &Mount,
    workspace_root: Option<&std::path::Path>,
    target: &SchemaRef,
    builtins: &[std::sync::Arc<memstead_schema::Schema>],
    registry_holds: impl FnOnce(&std::path::Path) -> bool,
) -> Result<(), crate::engine::EngineError> {
    let Some(gitdir) = lives_in_another_git_directory(mount, workspace_root) else {
        return Ok(());
    };
    let built_in = builtins.iter().any(|schema| {
        let (name, version) = schema.id();
        name == target.name && version == target.version
    });
    if built_in || registry_holds(gitdir) {
        return Ok(());
    }
    Err(crate::engine::EngineError::SchemaNotWhereTheMemLives {
        mem: mount.mem.clone(),
        pin: target.as_display(),
        gitdir: gitdir.display().to_string(),
    })
}

/// One `__MEMSTEAD:schemas/` registry a workspace resolves schema pins
/// from, and how much of it the workspace takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaRegistry {
    /// The git directory whose `__MEMSTEAD` ref carries the registry.
    pub gitdir: PathBuf,
    /// `None` for the workspace's own `mem-repo/.git`: every schema
    /// installed there resolves. `Some` for any other git directory:
    /// the workspace takes only the schemas that the mems it mounts
    /// from there pin, one entry per such mount.
    pub mounted: Option<Vec<MountedPins>>,
}

/// What one mount of another git directory contributes to the schemas
/// its workspace takes from that directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedPins {
    /// The mount's branch as a short name: the key of the mem's
    /// configuration on the registry ref (`mems/<branch>/config.json`),
    /// whose pin counts beside the roster's.
    pub branch: String,
    /// The pins the roster row carries: its `schema` and its
    /// `migration_target`, where set.
    pub pins: Vec<SchemaRef>,
}

/// The registries a workspace resolves schema pins from: the git
/// directory of every git-branch mount, in mount order, then the
/// workspace's own `mem-repo/.git` when it exists. Each directory
/// appears once (compared after canonicalisation, so a mount that names
/// the workspace's own mem-repo through another spelling does not
/// double).
///
/// A git-branch mem's schema is sealed on the registry of the
/// repository that holds the mem, which is the mount's git directory
/// and need not be the mounting workspace's `mem-repo/`: a workspace
/// may consist of engine state only and mount mems that live in
/// another workspace's mem-repo. From such a directory the workspace
/// takes what its mounts pin and nothing else ([`SchemaRegistry`]): a
/// schema that only a mem it does not mount pins is a statement about
/// that mem, and the workspace was not given the mem. Its own mem-repo
/// it reads whole, so the place a schema is installed into is a place
/// it is read from. On a name-and-version collision between two
/// registries the earlier directory wins.
pub fn schema_registries<'a>(
    mounts: impl IntoIterator<Item = &'a Mount>,
    workspace_root: Option<&std::path::Path>,
) -> Vec<SchemaRegistry> {
    let canonical = |gitdir: &PathBuf| gitdir.canonicalize().unwrap_or_else(|_| gitdir.clone());
    let own = workspace_root
        .map(|root| root.join("mem-repo").join(".git"))
        .filter(|gitdir| gitdir.is_dir());
    let own_key = own.as_ref().map(canonical);
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    let mut out: Vec<SchemaRegistry> = Vec::new();
    for mount in mounts {
        let MountStorage::GitBranch { gitdir, branch } = &mount.storage else {
            continue;
        };
        let key = canonical(gitdir);
        let is_own = own_key.as_ref() == Some(&key);
        let at = *index.entry(key).or_insert_with(|| {
            out.push(SchemaRegistry {
                gitdir: gitdir.clone(),
                mounted: (!is_own).then(Vec::new),
            });
            out.len() - 1
        });
        if let Some(mounted) = out[at].mounted.as_mut() {
            mounted.push(MountedPins {
                branch: branch_short_name(branch).to_string(),
                pins: mount
                    .schema
                    .iter()
                    .chain(mount.migration_target.iter())
                    .cloned()
                    .collect(),
            });
        }
    }
    if let (Some(gitdir), Some(key)) = (own, own_key)
        && !index.contains_key(&key)
    {
        out.push(SchemaRegistry {
            gitdir,
            mounted: None,
        });
    }
    out
}

/// The git directories of [`schema_registries`], for a reader that
/// fetches the files of a schema the workspace has already resolved
/// (an export, a health axis). A reader that resolves a name a caller
/// gave reads through [`schema_registries`] instead, which says how
/// much of each directory the workspace takes.
pub fn schema_registry_gitdirs<'a>(
    mounts: impl IntoIterator<Item = &'a Mount>,
    workspace_root: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    schema_registries(mounts, workspace_root)
        .into_iter()
        .map(|registry| registry.gitdir)
        .collect()
}

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

    fn branch_mount(mem: &str, gitdir: &std::path::Path, branch: &str) -> Mount {
        Mount {
            mem: mem.into(),
            schema: None,
            storage: MountStorage::GitBranch {
                gitdir: gitdir.to_path_buf(),
                branch: branch.into(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }
    }

    /// A workspace root with a `mem-repo/.git` of its own, and the git
    /// directory of another workspace's mem-repo beside it.
    fn own_and_other(tmp: &std::path::Path) -> (PathBuf, PathBuf, PathBuf) {
        let root = tmp.join("workspace");
        let own = root.join("mem-repo").join(".git");
        let other = tmp.join("store").join("mem-repo").join(".git");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        (root, own, other)
    }

    /// The workspace's own mem-repo is read whole (`mounted: None`),
    /// also when a mount names it through another spelling, and it
    /// appears once.
    #[test]
    fn schema_registries_reads_the_own_mem_repo_whole_under_any_spelling() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (root, own, _other) = own_and_other(tmp.path());
        let respelled = root
            .join("mem-repo")
            .join("..")
            .join("mem-repo")
            .join(".git");
        assert_ne!(respelled, own);

        let mut kept = branch_mount("kept", &respelled, "refs/heads/kept");
        kept.schema = Some(pin("widgets"));
        let other = branch_mount("other", &own, "other");
        let registries = schema_registries([&kept, &other], Some(root.as_path()));

        assert_eq!(
            registries,
            vec![SchemaRegistry {
                gitdir: respelled.clone(),
                mounted: None,
            }],
            "one registry, spelled as the first mount spells it, taken whole"
        );
        assert_eq!(
            schema_registry_gitdirs([&kept, &other], Some(root.as_path())),
            vec![respelled]
        );
    }

    /// Two mounts of one other git directory make one registry with one
    /// entry per mount, in mount order. An entry carries the branch as
    /// a short name and the roster's pins: `schema`, then
    /// `migration_target`. The own mem-repo comes last when no mount
    /// names it.
    #[test]
    fn schema_registries_groups_the_mounts_of_another_git_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (root, own, other) = own_and_other(tmp.path());
        let other_respelled = tmp
            .path()
            .join("store")
            .join(".")
            .join("mem-repo")
            .join(".git");

        let mut kept = branch_mount("kept", &other, "refs/heads/kept");
        kept.schema = Some(pin("widgets"));
        kept.migration_target = Some(pin("gadgets"));
        let folder = Mount {
            mem: "folder".into(),
            schema: Some(pin("default")),
            storage: MountStorage::Folder {
                path: PathBuf::from("folder"),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        // No pin on the roster row: the entry still stands, so that the
        // pin in the mem's configuration can count.
        let notes = branch_mount("notes", &other_respelled, "deep/notes");

        let registries = schema_registries([&kept, &folder, &notes], Some(root.as_path()));
        assert_eq!(
            registries,
            vec![
                SchemaRegistry {
                    gitdir: other.clone(),
                    mounted: Some(vec![
                        MountedPins {
                            branch: "kept".into(),
                            pins: vec![pin("widgets"), pin("gadgets")],
                        },
                        MountedPins {
                            branch: "deep/notes".into(),
                            pins: vec![],
                        },
                    ]),
                },
                SchemaRegistry {
                    gitdir: own.clone(),
                    mounted: None,
                },
            ]
        );

        // A workspace with no mem-repo of its own, and one with no root
        // at all, take every git directory by its mounts.
        let view = tmp.path().join("view");
        std::fs::create_dir_all(&view).unwrap();
        for workspace_root in [Some(view.as_path()), None] {
            let registries = schema_registries([&kept], workspace_root);
            assert_eq!(
                registries,
                vec![SchemaRegistry {
                    gitdir: other.clone(),
                    mounted: Some(vec![MountedPins {
                        branch: "kept".into(),
                        pins: vec![pin("widgets"), pin("gadgets")],
                    }]),
                }]
            );
        }

        // No mount at all: the own mem-repo alone, whole.
        assert_eq!(
            schema_registries(std::iter::empty::<&Mount>(), Some(root.as_path())),
            vec![SchemaRegistry {
                gitdir: own,
                mounted: None,
            }]
        );
    }

    /// A pin switch passes unasked for a mount of the workspace's own
    /// mem-repo and for other storage. For a mount of another git
    /// directory it passes for a built-in target and for a target that
    /// directory's registry holds, and refuses otherwise, naming the
    /// mem, the pin and the directory.
    #[test]
    fn a_pin_must_be_held_where_the_mem_lives() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (root, own, other) = own_and_other(tmp.path());
        let builtins = memstead_schema::builtins::load_builtin_schemas().unwrap();
        let (builtin_name, builtin_version) = builtins[0].id();
        let builtin = SchemaRef::new(&builtin_name, builtin_version);
        let custom = pin("widgets");
        let never = |_: &std::path::Path| -> bool { panic!("the registry is not asked") };

        let own_mount = branch_mount("kept", &own, "kept");
        assert_eq!(
            lives_in_another_git_directory(&own_mount, Some(root.as_path())),
            None
        );
        pin_is_held_where_the_mem_lives(&own_mount, Some(root.as_path()), &custom, &[], never)
            .unwrap();

        let folder = Mount {
            storage: MountStorage::Folder {
                path: PathBuf::from("folder"),
            },
            ..own_mount.clone()
        };
        assert_eq!(lives_in_another_git_directory(&folder, None), None);
        pin_is_held_where_the_mem_lives(&folder, None, &custom, &[], never).unwrap();

        let foreign = branch_mount("kept", &other, "kept");
        assert_eq!(
            lives_in_another_git_directory(&foreign, Some(root.as_path())),
            Some(other.as_path())
        );
        // The same git directory is another one for a workspace without
        // a root.
        assert_eq!(
            lives_in_another_git_directory(&own_mount, None),
            Some(own.as_path())
        );
        pin_is_held_where_the_mem_lives(&foreign, Some(root.as_path()), &builtin, &builtins, never)
            .unwrap();
        let mut asked = None;
        pin_is_held_where_the_mem_lives(
            &foreign,
            Some(root.as_path()),
            &custom,
            &builtins,
            |gitdir| {
                asked = Some(gitdir.to_path_buf());
                true
            },
        )
        .unwrap();
        assert_eq!(asked.as_deref(), Some(other.as_path()));

        let refused = pin_is_held_where_the_mem_lives(
            &foreign,
            Some(root.as_path()),
            &custom,
            &builtins,
            |_| false,
        )
        .unwrap_err();
        assert_eq!(refused.code(), "SCHEMA_NOT_FOUND");
        match &refused {
            crate::engine::EngineError::SchemaNotWhereTheMemLives { mem, pin, gitdir } => {
                assert_eq!(mem, "kept");
                assert_eq!(pin, "widgets@1.0.0");
                assert_eq!(gitdir, &other.display().to_string());
            }
            other => panic!("unexpected refusal: {other:?}"),
        }
        assert!(
            refused.to_string().contains("where the mem lives"),
            "{refused}"
        );
        assert_eq!(
            refused.details(),
            serde_json::json!({
                "mem": "kept",
                "pin": "widgets@1.0.0",
                "sources": [],
                "mem_git_directory": other.display().to_string(),
            })
        );
    }
}
