//! Mem configuration loading, validation, and top-level CRUD.
//!
//! Handles `.memstead/config.json` parsing, cross-field validation, and the
//! `update_config_field` write helper. Projections/mediums, their
//! validators, and the pre-rework migration have been dropped by the
//! workspace rewrite — `projections` / `mediums`
//! survive as unknown keys captured into `MemConfig.extra` so legacy
//! configs still round-trip, but the engine does not interpret them.
//!
//! Port of @memstead/config (config-contract.js, index.js) and
//! @agent-adapters/config-mcp (workspace.js).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The per-mem engine-internal directory under a folder mem's
/// root — `<mem_root>/.memstead/` holds `config.json` and
/// `changes.jsonl`. Defined here (rather than in `memstead-base`)
/// because mem-config loading lives in this crate and `memstead-base`
/// depends on it; `memstead-base` re-exports the constant for
/// downstream consumers. Distinct from the workspace store directory
/// (`memstead_base::WORKSPACE_STORE_DIR`) and from the in-zip member
/// paths inside sealed archives ([`ARCHIVE_META_DIR`]), which are a
/// separate on-disk format and never use this constant.
pub const MEM_META_DIR: &str = ".memstead";

// ---------------------------------------------------------------------------
// Sealed-archive surface constants
// ---------------------------------------------------------------------------
//
// A sealed archive is a zip whose engine-internal members live under one
// meta directory: `.memstead/config.json` plus the embedded schema tree
// `.memstead/schema/…` — the sole member layout. The file extension is
// `.mem` — the sole spelling, read and written. Defined here because
// this is the lowest crate every archive reader/writer (memstead-base,
// memstead-git-branch, memstead-registry, memstead-wasm, the CLIs)
// already depends on.

/// In-zip meta directory of a sealed archive — the only spelling.
pub const ARCHIVE_META_DIR: &str = ".memstead";
/// Member path of the published config inside a sealed archive.
pub const ARCHIVE_CONFIG_PATH: &str = ".memstead/config.json";
/// Member-path prefix of the embedded schema tree (manifest at
/// `<prefix>schema.yaml`, type files under `<prefix>types/`).
pub const ARCHIVE_SCHEMA_PREFIX: &str = ".memstead/schema/";
/// Member path of the optional authoring-provenance payload inside a
/// sealed archive (see [`crate::archive_provenance`]). Additive: archives
/// predating provenance omit it, and an engine that does not recognise it
/// tolerates it as an unknown meta member.
pub const ARCHIVE_PROVENANCE_PATH: &str = ".memstead/provenance.json";
/// Member path of the optional engine-owned anchors sidecar inside a
/// sealed archive (the provenance-anchor payload). Additive: archives
/// with no anchors omit it. Recognised as a first-class member so the
/// canonical re-pack threads it through verbatim rather than
/// silently stripping it (a recognised-but-malformed member is a typed
/// validation failure, unlike unknown future meta members which stay
/// tolerate-and-ignore).
pub const ARCHIVE_ANCHORS_PATH: &str = ".memstead/anchors.json";
/// File extension (without dot) of a sealed archive — the sole spelling.
/// The one deliberately-distinct token in a project that is otherwise
/// "memstead" everywhere — short, and derived from the project name.
pub const ARCHIVE_EXTENSION: &str = "mem";

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config file not found: {0}")]
    NotFound(String),
    #[error("invalid JSON in config file: {0}")]
    InvalidJson(String),
    #[error("config validation failed:\n{}", .0.iter().map(|e| format!("  - {e}")).collect::<Vec<_>>().join("\n"))]
    ValidationFailed(Vec<String>),
    #[error("{0}")]
    Other(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Config check result
// ---------------------------------------------------------------------------

/// Result of config validation — errors are fatal, warnings are informational.
#[derive(Debug, Clone)]
pub struct ConfigCheckResult {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Stable `UPPER_SNAKE_CASE` envelope code when the validator
    /// detects a categorical failure that callers should branch on.
    /// Currently set to `"LEGACY_FIELD_PRESENT"` when any entry in
    /// `LEGACY_TOMBSTONE_KEYS` is present.
    pub error_code: Option<String>,
}

// ---------------------------------------------------------------------------
// Mem config types (deserialized from .memstead/config.json)
// ---------------------------------------------------------------------------

/// Role-based publish config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleConfig {
    pub include: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

/// Publish config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishConfig {
    pub roles: HashMap<String, RoleConfig>,
}

/// Community detection override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
}

/// One entry in the legacy `MemConfig.read_mems` map — a read-only
/// sealed mem archive that a pre-2026-08 install attached to a host
/// mem. Retained only for the one-way boot migration into the mount
/// roster; no live path writes one.
///
/// The engine resolves each entry to a cache file: when `cache_key` is
/// present, `<mem_cache_dir>/<name>-<cache_key>.mem` (content-addressed
/// — see [`ReadMemSpec::cache_key`]); otherwise the legacy
/// `<mem_cache_dir>/<name>.mem`.
///
/// Kept as a struct (rather than collapsing to a bare `ReadMemSource`)
/// so forward-compatible fields can be added without another schema break.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadMemSpec {
    pub source: ReadMemSource,
    /// Content-address of the installed archive — a short hex digest of
    /// the validator's canonical bytes. The install path writes the cache
    /// file at `<cache>/<name>-<cache_key>.mem`, so two distinct archives
    /// sharing an internal mem name land in distinct files (no collision)
    /// and re-installing identical bytes resolves to the same file (dedup).
    /// `None` for legacy registrations written before content-addressing;
    /// the loader then falls back to the bare `<name>.mem` path.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "cacheKey")]
    pub cache_key: Option<String>,
}

/// How the app reconstitutes a read mem's cache file when missing.
///
/// The engine itself never fetches; `source` is metadata consumed by the
/// app's installer. A `Registry` variant with scope/name identifiers
/// will be added once the memstead.io registry ships.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ReadMemSource {
    /// User dropped an archive file onto the app. App cannot auto-reinstall —
    /// it prompts the user to drop the original file again.
    Local,
    /// Fetched from an HTTPS URL (GitHub Releases, shared drive, any static
    /// host). Engine-side no-op; the app's installer re-fetches on attach.
    Url { url: String },
    // `Registry` variant reserved for when the memstead.io registry ships.
    // The exact shape (fields, id format like `@scope/name`) is designed
    // then — declaring it up front without semantics would be
    // speculative, and pre-1.0 adding a variant later is not a breaking
    // change for anyone.
}

/// Reference to a schema by exact name and version — `name@x.y.z`.
///
/// Serializes/deserializes as a single string so mem configs read
/// `{ "schema": "default@1.0.0" }` on disk. Range syntax (`^`, `~`,
/// `latest`) is rejected — schema pinning is strict and explicit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaRef {
    pub name: String,
    pub version: semver::Version,
}

impl SchemaRef {
    pub fn new(name: impl Into<String>, version: semver::Version) -> Self {
        Self {
            name: name.into(),
            version,
        }
    }

    pub fn as_display(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

impl std::fmt::Display for SchemaRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

impl std::str::FromStr for SchemaRef {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("schema reference must not be empty (expected \"name@x.y.z\")".into());
        }
        let (name, version_str) = trimmed.split_once('@').ok_or_else(|| {
            format!(
                "schema reference '{trimmed}' must include an exact version — expected \"name@x.y.z\""
            )
        })?;
        if name.is_empty() {
            return Err("schema reference name must not be empty".into());
        }
        if version_str == "latest" {
            return Err(format!(
                "schema reference '{trimmed}' uses 'latest' — exact semver versions only"
            ));
        }
        if version_str.starts_with(['^', '~', '>', '<', '=', '*']) {
            return Err(format!(
                "schema reference '{trimmed}' uses range syntax — exact semver only (e.g. 'default@1.0.0')"
            ));
        }
        let version = semver::Version::parse(version_str).map_err(|e| {
            format!("schema reference '{trimmed}' has invalid semver version '{version_str}': {e}")
        })?;
        Ok(Self {
            name: name.to_string(),
            version,
        })
    }
}

impl Serialize for SchemaRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_display())
    }
}

impl<'de> Deserialize<'de> for SchemaRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse::<SchemaRef>().map_err(serde::de::Error::custom)
    }
}

// `MemSchemaPin` (the two-variant pin with a name-only fallback) was
// retired here. Mem configs now declare a strict `<name>@<version>`
// pin parsed directly through [`SchemaRef`]; bare-name pins are
// rejected at config load.

/// VCS layout for a writable mem — optional `{ gitdir, worktree }` pair
/// in `.memstead/config.json`. When absent, the engine resolves the default:
/// `.git/` at mem root with `.` as worktree.
///
/// Paths are relative to mem root and interpreted by `memstead-git-branch` —
/// this crate just carries them through serde. Masterplan §3.4 is
/// explicit that the primitive is a pair of paths; the two canonical
/// idioms (isolated `{ ".git", "." }` and shared `{ "../.git", ".." }`)
/// are idioms, not enum variants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VcsConfig {
    /// Path to the gitdir relative to mem root. Required when the
    /// `vcs` block is present.
    pub gitdir: String,
    /// Path to the worktree relative to mem root. Optional within the
    /// `vcs` block — defaults to `"."` (mem root) when omitted.
    #[serde(default = "vcs_worktree_default")]
    pub worktree: String,
}

fn vcs_worktree_default() -> String {
    ".".to_string()
}

/// Tolerant deserializer for the `vcs` field: accepts the object form
/// (`{ gitdir, worktree? }`) and returns `None` for any non-object value
/// (string, number, boolean, null). A missing field is also `None`.
///
/// Motivation: an older macOS Mem-mode UI wrote `"vcs": "system"`
/// (and similar sentinel strings) into `.memstead/config.json` files that
/// now must continue to load without editing those files by hand.
/// Strict validation of the object form — unknown keys, missing
/// `gitdir`, etc. — still surfaces as a hard serde error.
fn deserialize_vcs_tolerant<'de, D>(deserializer: D) -> Result<Option<VcsConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(_)) => {
            let v = value.unwrap();
            Ok(Some(
                serde_json::from_value(v).map_err(serde::de::Error::custom)?,
            ))
        }
        Some(_) => Ok(None),
    }
}

/// The subject block of a mem — what it covers, by what method, and
/// above all what was considered and deliberately left out. Exactly
/// three members, by design: every additional slot invites the
/// working-notes leakage this block exists to avoid, and three is what
/// a recipient can actually read before deciding whether to trust the
/// mem. Published verbatim in [`PublishedMemConfig`]; the engine never
/// parses, links, or validates the prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MemSubject {
    /// What this mem covers.
    pub scope: String,
    /// How its content was arrived at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// What was considered and deliberately left out — prose
    /// statements, order preserved. May be empty.
    #[serde(default)]
    pub exclusions: Vec<String>,
}

/// Engine-owned version stamp of the last successful mutation on a
/// mem — see [`MemConfig::mutation_stamp`]. Both values are recorded
/// at mutation time: `engine_version` is the engine crate version the
/// acting binary was built from, `schema` the resolved
/// `<name>@<version>` the mutation validated against (the resolved
/// schema, not merely the pin — mid-migration mems stamp the target
/// they validated against).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MutationStamp {
    /// Full build version of the binary that performed the last
    /// mutation — `memstead-base`'s `build_info::full_version()`:
    /// the crate semver, plus `+g<sha>[-dirty]` build metadata when
    /// built inside a git checkout, so dev builds between releases
    /// stamp distinguishably. Older stamps carry the plain semver
    /// and compare against the full current value — firing the skew
    /// hint on the first post-upgrade boot is desired, no migration.
    pub engine_version: String,
    /// Resolved schema `<name>@<version>` the last mutation validated
    /// against.
    pub schema: String,
}

/// Full mem configuration loaded from .memstead/config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemConfig {
    /// Workspace mem-config format version. Absent means version 1 — the
    /// healthy common case; real mems carry no key — and an unknown value
    /// REFUSES at parse, matching every sibling store (the binding record,
    /// `WorkspaceConfig`, the anchors sidecar). Until 2026-08-28 this field
    /// was not modeled at all, so serde silently dropped whatever value a
    /// config carried and `"format": 99` verified clean. Distinct from
    /// [`PUBLISHED_MEM_FORMAT`], the sealed-archive config's own version
    /// line with its own gate. Read through [`Self::format_version`].
    #[serde(
        default,
        deserialize_with = "de_mem_config_format",
        skip_serializing_if = "Option::is_none"
    )]
    pub format: Option<u32>,

    /// Optional mem name. The leaf folder name under
    /// `__MEMSTEAD:mems/` (and the disk basename on the legacy disk
    /// path) is authoritative; engine-written configs omit this
    /// field. Tolerated on read for pre-cutover configs and for the
    /// [`PublishedMemConfig`] conversion path that still requires
    /// an explicit identity (the caller passes the name in when
    /// projecting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Semver version of the mem content. Read at mem-archive export
    /// time so the engine always knows the current version without manual
    /// tracking. Parsed at config load — invalid version strings fail fast
    /// with a source-attributed serde error rather than slipping through to
    /// export (where the issue only surfaces when a downstream loader tries
    /// to resolve a `semver::VersionReq` against the mem).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<semver::Version>,

    /// One-line description of the mem, surfaced in mem-archive metadata
    /// and UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Human-readable display title. Display text, NOT identity: no
    /// slug grammar, no uniqueness rule — the mem name stays the sole
    /// handle everywhere (paths, grants, namespace patterns, cross-mem
    /// references, archive filenames). Surfaces that print a mem
    /// prefer this and fall back to the name. Published in
    /// [`PublishedMemConfig`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The mem's subject — scope, method, deliberate exclusions.
    /// Published verbatim; clears as a unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<MemSubject>,

    /// Optional author attribution, surfaced in mem-archive metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,

    /// Declared process-mem pairing: the name
    /// of the mem holding this mem's process tier (verification
    /// targets, findings, inquiry entries). Declaration wins over the
    /// binding-name derivation the brief renderer and the
    /// open-questions health axis otherwise use; a declaration naming
    /// an unmounted mem surfaces as a typed health finding, never a
    /// silent fallback. Absent means "derive by convention" — the
    /// pre-declaration behaviour, unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_mem: Option<String>,

    /// Schema this mem is pinned to. Exact `<name>@<version>` pin
    /// only — bare-name forms are rejected at config load. Exactly one
    /// schema per mem. The `Option` keeps serde tolerant so a missing
    /// key surfaces as a structured error from `check_config` rather
    /// than a deserialize panic; a `None` value is a validation error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaRef>,

    /// Opaque string-map passed through by the engine. Agents and
    /// plugin prompt renderers are free to invent their own keys; the
    /// engine does not parse, validate, or interpret any value inside.
    /// Stripped from `PublishedMemConfig` — guidance is workspace-
    /// local authorship metadata, not part of the published identity.
    ///
    /// Pre-2026-04-24 this field was `Option<Value>`; the workspace
    /// rewrite normalised it to a map so the shape on
    /// the wire is stable and the engine's pass-through guarantee is
    /// type-checked.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub write_guidance: HashMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish: Option<PublishConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// **Legacy since 2026-08.** A read-only sealed-archive mem attaches
    /// to the WORKSPACE mount roster (`.memstead/state/mounts.json`), not
    /// to a host mem's config. Nothing writes this field any more; it
    /// survives on the struct so the one-way boot migration
    /// (`migrate_legacy_read_mems`) can recognise a pre-cutover
    /// registration, turn it into a mount, and strip the key. An empty or
    /// omitted map is the healthy state.
    ///
    /// Key is the mem name (matching the archive's config name).
    ///
    /// `BTreeMap` (not `HashMap`) so iteration and serialization order
    /// are stable — reproducible log output and diff-friendly config on
    /// disk. Explicit `rename = "readMems"` documents the on-disk name
    /// at the field (the struct-level `rename_all = "camelCase"` already
    /// handles it, but explicit rename is greppable from either side).
    #[serde(
        rename = "readMems",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub read_mems: BTreeMap<String, ReadMemSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community: Option<CommunityOverride>,

    /// Optional VCS layout override. When absent, `memstead-git-branch` resolves
    /// the default at init time: `.git/` at mem root with `.` as
    /// worktree. When present, `gitdir` and `worktree` are paths
    /// relative to the mem root. Stripped from `PublishedMemConfig`
    /// — VCS layout is workspace-local mechanics, not part of the
    /// published mem's identity.
    ///
    /// Deserialization is tolerant of legacy non-object values (e.g.
    /// `"vcs": "system"` — the sentinel an older macOS Mem-mode
    /// UI wrote): any non-object form deserializes to `None` and falls
    /// back to the default-resolution path. The object form is validated
    /// strictly.
    #[serde(
        default,
        deserialize_with = "deserialize_vcs_tolerant",
        skip_serializing_if = "Option::is_none"
    )]
    pub vcs: Option<VcsConfig>,

    /// Tombstone marker written by `memstead mem unregister`. ISO-8601
    /// UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) recorded at the moment
    /// the mem was unregistered while its storage was preserved.
    /// When `memstead mem init <same-name>`
    /// probes the storage and finds an `unregistered_at` value, it
    /// treats the residue as deliberate operator state and defaults
    /// to the `Reattach` recovery action (adopting the preserved
    /// entities and clearing the tombstone). Absence (`None`) on
    /// otherwise-present residue triggers `MEM_STORAGE_RESIDUE_DETECTED`
    /// unless the caller passes an explicit `recovery` flag. Stripped
    /// from `PublishedMemConfig` — tombstones are workspace-local
    /// lifecycle state, not part of the published mem's identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unregistered_at: Option<String>,

    /// Per-source "last successfully synced source state", written by
    /// the ingest layer and surfaced verbatim on the workspace dump.
    /// The engine never parses, validates, or interprets a value:
    /// each token is opaque, its meaning owned by the medium-type
    /// layer that produced it (git → commit id, graph → snapshot
    /// token, filesystem → a small stat digest the plugin
    /// JSON-stringifies). The key is likewise opaque — the binding
    /// layer keys per `(binding, facet)` (conventionally
    /// `"<binding-id>/<facet>#synced"`, D4), but the engine treats it as
    /// an arbitrary string. This is the durable, shared baseline against which a
    /// fresh ingest iteration diffs "what changed since last time";
    /// it survives a skill-cache wipe and a machine change because it
    /// lives in engine-held mem config, not ephemeral plugin cache.
    ///
    /// Stripped from `PublishedMemConfig` — sync state is
    /// workspace-local ingest bookkeeping, not part of a published
    /// mem's identity. `BTreeMap` (not `HashMap`) for stable
    /// serialization order: diff-friendly config on disk and
    /// reproducible dump output.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sync_state: BTreeMap<String, String>,

    /// The mem's review mark: the last human-approved state, in the
    /// backend-opaque cursor vocabulary `changes_since` consumes
    /// (git-branch: commit SHA; folder: changelog RFC3339 timestamp).
    /// Absent is a first-class state — a mem with no mark is ordinary,
    /// never an error, and marks never gate writes. One mark per mem;
    /// wire key `reviewMark` (camelCase per the config's convention).
    ///
    /// Stripped from `PublishedMemConfig` (allowlist projection) —
    /// review state is workspace-collaboration bookkeeping, not part
    /// of a published mem's identity.
    #[serde(
        default,
        rename = "reviewMark",
        skip_serializing_if = "Option::is_none"
    )]
    pub review_mark: Option<String>,

    /// Engine-owned version stamp of the last successful mutation:
    /// which engine version and which resolved schema performed it.
    /// Written by the engine after a mutation and only when the values
    /// changed (a binary upgrade or a schema repin), never by authors,
    /// never on read-only loads — a boot writes nothing. Boot compares
    /// the running binary against the stamp and surfaces a divergence
    /// as the warn-tier `ENGINE_VERSION_SKEW` hint; absence of a stamp
    /// is a first-class state (pre-stamp mems), never skew. The stamp
    /// is the substrate any future migration machinery would consult —
    /// deliberately built without that machinery.
    ///
    /// Stripped from `PublishedMemConfig` (allowlist projection) —
    /// workspace-local engine bookkeeping, not published identity.
    /// Wire key `mutationStamp` (camelCase per the config convention).
    #[serde(
        default,
        rename = "mutationStamp",
        skip_serializing_if = "Option::is_none"
    )]
    pub mutation_stamp: Option<MutationStamp>,

    /// Extra fields not in the known set (captured for round-tripping).
    ///
    /// Historical tombstones:
    /// - `defaultSchema` (pre-2026-04): legacy per-mem default type.
    ///   Per-entity `type:` frontmatter is authoritative now.
    /// - `types: [...]` (pre-schema-artifact, 2026-04): replaced by
    ///   `schema: "<name>@<version>"`. Legacy entries are hard-rejected
    ///   by `check_config`.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// Published (archive) mem config
// ---------------------------------------------------------------------------

/// Strict-ingress shape of a mem config. This is the **only** metadata
/// form that enters a `.mem` archive. `MemConfig` carries author-only
/// fields (writeGuidance, rules, publish, readMems, language,
/// community, defaultSchema, vcs, plus any key captured in
/// `extra`) that never belong in a published archive;
/// `published_config_from` projects `MemConfig` →
/// `PublishedMemConfig`, dropping everything outside the whitelist.
///
/// `deny_unknown_fields` + no `serde(flatten)` on purpose: the validator
/// re-parses this shape with the same struct as defense-in-depth, so any
/// legacy author key smuggled into an archive surfaces as a rejection
/// instead of a silently-tolerated payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PublishedMemConfig {
    pub format: u32,
    pub name: String,
    pub version: semver::Version,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Human-readable display title (format ≥ 4). Display text, not
    /// identity — a format-3 archive simply has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The mem's subject block (format ≥ 4), published verbatim —
    /// exclusions included and in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<MemSubject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    pub schema: SchemaRef,
}

/// Archive format integer written to the archive config's `format`
/// field. Bumped to `4` for the mem title + subject block (both
/// optional — a format-3 archive simply has neither). Readers accept
/// `3` and `4` via [`published_format_accepted`]; `format: 1` (V1) and
/// `format: 2` (V2, top-level `schema/` tree) archives keep refusing
/// cleanly.
pub const PUBLISHED_MEM_FORMAT: u32 = 4;

/// The workspace mem config's format version (`<mem>/.memstead/config.json`).
/// One accepted value; an absent key means exactly this version, and any
/// other value refuses at parse ([`de_mem_config_format`]).
pub const MEM_CONFIG_FORMAT: u32 = 1;

/// Deserialize the workspace mem config's `format` key: absent stays `None`
/// (meaning [`MEM_CONFIG_FORMAT`]), the current version parses, anything
/// else refuses loudly — a config a future engine may mean differently must
/// never be silently read as today's shape.
fn de_mem_config_format<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<u32>::deserialize(deserializer)?;
    match value {
        None | Some(MEM_CONFIG_FORMAT) => Ok(value),
        // A mounted read-mem's cached config is the published shape and
        // parses through this same struct, so the published versions this
        // engine reads are accepted here too (their own gate ran at
        // archive validation).
        Some(found) if published_format_accepted(found) => Ok(value),
        Some(found) => Err(serde::de::Error::custom(format!(
            "unsupported mem config format {found}: this engine reads format \
             {MEM_CONFIG_FORMAT} (an absent key means the same); a higher value \
             was written by a newer engine — upgrade before loading this mem"
        ))),
    }
}

impl MemConfig {
    /// The effective config format version: the declared value, or
    /// [`MEM_CONFIG_FORMAT`] when the key is absent.
    pub fn format_version(&self) -> u32 {
        self.format.unwrap_or(MEM_CONFIG_FORMAT)
    }
}

/// Every archive format a current reader accepts, newest first: the
/// current integer plus format 3 (the pre-title/subject shape — the two
/// mems already published on the live registry stay installable without
/// a re-publish). `format: 1` (V1) and `format: 2` (V2, top-level
/// `schema/` tree) keep refusing cleanly.
///
/// A list rather than a predicate body because the accepted set is a
/// fact worth reading directly, not only a branch to evaluate.
pub const PUBLISHED_MEM_FORMATS_ACCEPTED: &[u32] = &[PUBLISHED_MEM_FORMAT, 3];

/// Does a reader updated for the current format accept an archive at
/// `format`? The single predicate every reader gate consults —
/// validation (`validator::config::parse_config_bytes`) and byte
/// hydration (`Engine::from_archive_bytes`) alike — so acceptance
/// cannot drift between them.
pub fn published_format_accepted(format: u32) -> bool {
    PUBLISHED_MEM_FORMATS_ACCEPTED.contains(&format)
}

/// Errors returned by `published_config_from`. Actionable messages —
/// the caller (export pipeline, publish pipeline) surfaces these
/// directly to the user without wrapping a raw serde error.
#[derive(Debug, thiserror::Error)]
pub enum PublishConversionError {
    #[error("config.version is required for mem publish — set it in .memstead/config.json")]
    MissingVersion,
    #[error(
        "config must declare `schema` (e.g. \"default@1.0.0\") — set it in .memstead/config.json"
    )]
    MissingSchema,
    #[error(
        "publish requires an explicit mem name — caller must pass the leaf folder name (Goal 3 of mem-repo-restructure dropped the in-config `name` requirement)"
    )]
    MissingName,
}

/// The whitelist projection. Everything author-only is discarded; only
/// the fields that make sense outside the author's working directory
/// ride into the archive. `format` is pinned at `PUBLISHED_MEM_FORMAT`.
///
/// `name` is supplied explicitly by the caller — the on-disk `name`
/// field is optional and the engine no longer treats it as the
/// mem-identity source. The published archive still needs an
/// identity, so the publishing path passes the leaf folder name
/// (`__MEMSTEAD:mems/<path>/<leaf>/config.json`'s `<leaf>`, or the
/// disk basename on the legacy disk path) here. Falls back to the
/// in-config `name` field when the caller passes an empty string and
/// the config still carries a legacy `name` value (so pre-cutover
/// archives published before the migration land cleanly).
pub fn published_config_from(
    config: &MemConfig,
    name: &str,
) -> Result<PublishedMemConfig, PublishConversionError> {
    let version = config
        .version
        .clone()
        .ok_or(PublishConversionError::MissingVersion)?;
    let schema = config
        .schema
        .clone()
        .ok_or(PublishConversionError::MissingSchema)?;
    let resolved_name = if name.is_empty() {
        config
            .name
            .clone()
            .ok_or(PublishConversionError::MissingName)?
    } else {
        name.to_string()
    };
    Ok(PublishedMemConfig {
        format: PUBLISHED_MEM_FORMAT,
        name: resolved_name,
        version,
        description: config.description.clone(),
        title: config.title.clone(),
        subject: config.subject.clone(),
        authors: config.authors.clone(),
        schema,
    })
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const KNOWN_TOP_LEVEL_KEYS: &[&str] = &[
    "version",
    "description",
    "title",
    "subject",
    "authors",
    "schema",
    "writeGuidance",
    "rules",
    "publish",
    "language",
    "readMems",
    "community",
    "vcs",
    "syncState",
];

/// Keys that are explicitly rejected with a `LEGACY_FIELD_PRESENT`
/// envelope when present in a config. The validator surfaces a hard
/// error (not a soft "unknown key" warning) so agents that recreate
/// the legacy shape from training-set examples see a structured
/// rejection instead of silent acceptance with drift.
///
/// Each entry pairs the rejected key with an actionable error message.
/// New tombstones land here when a top-level key migrates from
/// "deprecated but tolerated" to "must not be re-authored". The table
/// holds entries for `name` (the field is now path-derived),
/// `types: [...]` (the pre-existing tombstone, preserved verbatim),
/// `belongsTo` (cross-mem authorization moved to the workspace file),
/// and `deps` (cross-mem attachments moved to the mount roster).
const LEGACY_TOMBSTONE_KEYS: &[(&str, &str)] = &[
    (
        "types",
        "Legacy `types: [...]` field detected — replace with `schema: \"<name>@<version>\"` \
         (e.g. `\"schema\": \"default@1.0.0\"`).",
    ),
    (
        "name",
        "Legacy `name` field detected — the mem leaf folder under `__MEMSTEAD:mems/` (or the \
         disk basename on the legacy disk path) is path-derived under the unified layout; \
         remove the field from `.memstead/config.json`.",
    ),
    (
        "deps",
        "Legacy `deps` field detected — a cross-mem attachment is a mount now, not a config \
         list. Remove the field from `.memstead/config.json` and run `memstead install \
         <scope>/<name>` so the attachment lands in \
         `.memstead/state/mounts.json`, where the engine reads it.",
    ),
    (
        "belongsTo",
        "Legacy `belongsTo` field detected — cross-mem authorization moved to the \
         workspace-level `[cross_mem_links]` section in `.memstead/workspace.toml`. Remove the \
         field from `.memstead/config.json` and add an entry under `[cross_mem_links]` \
         instead.",
    ),
];

// ---------------------------------------------------------------------------
// checkConfig — the main validator
// ---------------------------------------------------------------------------

/// Validate a raw config JSON value. Returns structured errors and warnings.
pub fn check_config(config: &Value) -> ConfigCheckResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let obj = match config.as_object() {
        Some(o) => o,
        None => {
            errors.push("(root): config must be an object".to_string());
            return ConfigCheckResult {
                valid: false,
                errors,
                warnings,
                error_code: None,
            };
        }
    };

    // 2. Legacy tombstones — keys that must not be re-authored. Each
    //    hit produces a hard error and pins the `LEGACY_FIELD_PRESENT`
    //    envelope code so callers branch on a stable identifier rather
    //    than the human-readable error message. See
    //    `LEGACY_TOMBSTONE_KEYS` for the reject list.
    let mut legacy_field_hit = false;
    for (key, message) in LEGACY_TOMBSTONE_KEYS {
        if obj.contains_key(*key) {
            errors.push((*message).to_string());
            legacy_field_hit = true;
        }
    }

    // 2b. Format version: absent means MEM_CONFIG_FORMAT; the current
    //     version passes; anything else is a hard error — the same verdict
    //     the parse-time gate renders, carried here so the validation
    //     surface (CLI check output) names it too.
    match obj.get("format") {
        None => {}
        Some(v) if v.as_u64() == Some(u64::from(MEM_CONFIG_FORMAT)) => {}
        Some(v)
            if v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .is_some_and(published_format_accepted) => {}
        Some(v) => errors.push(format!(
            "format: unsupported mem config format {v}: this engine reads format \
             {MEM_CONFIG_FORMAT} (an absent key means the same); a higher value was \
             written by a newer engine — upgrade before loading this mem"
        )),
    }

    // 3. Schema field: exact `name@x.y.z` reference required. Bare-name
    //    pins are rejected at parse time via SchemaRef::from_str.
    match obj.get("schema") {
        Some(Value::String(s)) => {
            if let Err(e) = s.parse::<SchemaRef>() {
                errors.push(format!("schema: {e}"));
            }
        }
        Some(_) => errors.push(
            "schema: must be a string of the form \"<name>@<x.y.z>\" \
             (exact version pin, e.g. \"default@1.0.0\")"
                .to_string(),
        ),
        None => errors.push(
            "Config must declare `schema` — exact pin of the form \
             \"<name>@<x.y.z>\" (e.g. \"default@1.0.0\")"
                .to_string(),
        ),
    }

    // 4. Read-mems map — source presence and shape.
    //    Cache-file existence is checked at engine init (`Engine::init`
    //    via `mem_cache`), not here, so isolated schema tests don't
    //    need real archive fixture files. The cached archive's config is
    //    authoritative for the version; no `version` or `path` is
    //    recorded in the config entry.
    if let Some(Value::Object(mems)) = obj.get("readMems") {
        for (name, spec) in mems {
            let entry_path = format!("readMems.{name}");

            let spec_obj = match spec.as_object() {
                Some(o) => o,
                None => {
                    errors.push(format!("{entry_path}: read-mem entry must be an object"));
                    continue;
                }
            };

            let source = match spec_obj.get("source").and_then(|v| v.as_object()) {
                Some(s) => s,
                None => {
                    errors.push(format!(
                        "{entry_path}.source: read-mem entry must declare a source \
                         (e.g. {{\"type\": \"local\"}} or {{\"type\": \"url\", \"url\": \"…\"}})"
                    ));
                    continue;
                }
            };

            match source.get("type").and_then(|v| v.as_str()) {
                Some("local") => {}
                Some("url") => match source.get("url").and_then(|v| v.as_str()) {
                    Some(u) if !u.is_empty() => {}
                    _ => errors.push(format!(
                        "{entry_path}.source.url: url source must declare a non-empty 'url' string"
                    )),
                },
                // `registry` type is reserved for future use but not
                // accepted yet — fails here alongside any other unknown.
                Some(other) => errors.push(format!(
                    "{entry_path}.source.type: unknown source type '{other}' \
                     (expected 'local' or 'url')"
                )),
                None => errors.push(format!(
                    "{entry_path}.source.type: source must declare a 'type' \
                     ('local' or 'url')"
                )),
            }
        }
    }

    // 5. `belongsTo` is now a tombstone (see `LEGACY_TOMBSTONE_KEYS`).
    //    Cross-mem authorization moved to the workspace-level
    //    `[cross_mem_links]` section in `.memstead/workspace.toml`. Per-mem config
    //    blobs that still carry the field are rejected with the
    //    tombstone error above; no shape validation runs here.

    // 7. Unknown key warnings. Tombstone keys are rejected above and
    //    skipped here so callers don't see a redundant warning alongside
    //    the hard error.
    for key in obj.keys() {
        if KNOWN_TOP_LEVEL_KEYS.contains(&key.as_str())
            || LEGACY_TOMBSTONE_KEYS
                .iter()
                .any(|(k, _)| *k == key.as_str())
        {
            continue;
        }
        warnings.push(format!(
            "Unknown config key '{key}' \u{2014} will be ignored"
        ));
    }

    let error_code = if legacy_field_hit {
        Some("LEGACY_FIELD_PRESENT".to_string())
    } else {
        None
    };

    ConfigCheckResult {
        valid: errors.is_empty(),
        errors,
        warnings,
        error_code,
    }
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

/// Load and parse a config from a mem directory.
/// Reads `<mem_dir>/.memstead/config.json`.
pub fn load_config(mem_dir: &Path) -> Result<(Value, PathBuf), ConfigError> {
    let config_path = mem_dir.join(MEM_META_DIR).join("config.json");
    let raw = std::fs::read_to_string(&config_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ConfigError::NotFound(config_path.display().to_string())
        } else {
            ConfigError::Io(e)
        }
    })?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|_| ConfigError::InvalidJson(config_path.display().to_string()))?;
    Ok((parsed, config_path))
}

/// Parse a raw JSON value into a MemConfig.
pub fn parse_mem_config(value: &Value) -> Result<MemConfig, ConfigError> {
    serde_json::from_value(value.clone()).map_err(|e| ConfigError::Other(e.to_string()))
}

/// Load, validate, and parse a mem config from disk.
pub fn load_and_validate(mem_dir: &Path) -> Result<MemConfig, ConfigError> {
    let (raw, _path) = load_config(mem_dir)?;

    let result = check_config(&raw);
    if !result.valid {
        return Err(ConfigError::ValidationFailed(result.errors));
    }

    parse_mem_config(&raw)
}

// ---------------------------------------------------------------------------
// Config writing
// ---------------------------------------------------------------------------

/// Write a config JSON value to disk (pretty-printed with trailing newline).
fn write_config(config_path: &Path, config: &Value) -> Result<(), ConfigError> {
    let json = serde_json::to_string_pretty(config)? + "\n";
    std::fs::write(config_path, json)?;
    Ok(())
}

/// Validate and write a config to disk. Returns check result.
fn commit_config(
    config_path: &Path,
    config: &Value,
    dry_run: bool,
) -> Result<ConfigCheckResult, ConfigError> {
    let check = check_config(config);
    if !check.valid {
        return Ok(check);
    }
    if !dry_run {
        write_config(config_path, config)?;
    }
    Ok(check)
}

// ---------------------------------------------------------------------------
// Config CRUD operations
// ---------------------------------------------------------------------------

/// Allowed top-level fields for `update_config_field`. The
/// workspace rewrite dropped `mediums` and
/// `projections` here: the engine no longer recognises those blocks so
/// they are not writable through the update surface either.
const ALLOWED_UPDATE_FIELDS: &[&str] = &[
    "version",
    "description",
    "authors",
    "writeGuidance",
    "rules",
    "readMems",
    "schema",
    "language",
    "publish",
];

const PROTECTED_FIELDS: &[&str] = &["name"];

/// Update a top-level config field.
pub fn update_config_field(
    config_path: &Path,
    config: &mut Value,
    field: &str,
    value: Value,
    dry_run: bool,
) -> Result<ConfigCheckResult, ConfigError> {
    if PROTECTED_FIELDS.contains(&field) {
        return Err(ConfigError::Other(format!("Field '{field}' is protected")));
    }

    let obj = config
        .as_object_mut()
        .ok_or_else(|| ConfigError::Other("config must be an object".into()))?;

    if !ALLOWED_UPDATE_FIELDS.contains(&field) {
        return Err(ConfigError::Other(format!(
            "Field '{field}' is not a recognized config field. Allowed: {}",
            ALLOWED_UPDATE_FIELDS.join(", ")
        )));
    }

    obj.insert(field.to_string(), value);
    commit_config(config_path, config, dry_run)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
